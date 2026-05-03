# 0009 — FEA: Skill + tool registry + orchestrator for local agents

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0008

## Problem
After #0008, the storage substrate exists (Manuscript, Author Layer with typed accessors, Agent Layer, proposal queue). What does not yet exist is a unified way for *agents* — local LLM invocations against gemma4 via Ollama — to call into that substrate.

The four current coach pipelines (voice/show/prose/spelling) each hand-build their prompt, hit Ollama, parse a response, and write directly into book state. They share no notion of:

- A **tool registry** the model can call from inside a generation.
- A **declarative skill** that names its inputs, allowed tools, and output shape so the orchestrator can validate and persist.
- A **trust boundary** that prevents a skill from writing to the Author Layer (per `docs/memory-architecture.md` §4.4 — Author Layer mutations must go through `author.propose`).
- **Always-loaded context assembly** (entity bible + chapter rollups + current chapter; ticket #0011 ships the assembly, this ticket ships the integration point).
- **Per-skill logging**: token counts, tool call traces, latency, model id.

Without this framework, every new skill (#0010 transcoder, #0011 rollup, future continuity-check / voice-drift / thread-tracker) re-implements the same plumbing inconsistently, and the agent has no enforced way to stay inside its sandbox.

See `docs/memory-architecture.md` §4 for the design.

## Scope

### Module layout
- New module tree `src/agent/`:
  - `src/agent/mod.rs` — public surface (`Skill`, `Tool`, `Orchestrator`, `Invocation`, `InvocationLog`)
  - `src/agent/tools.rs` — `Tool` trait, the initial tool registry, permission tags
  - `src/agent/skills.rs` — `Skill` struct + `SkillRegistry`
  - `src/agent/orchestrator.rs` — `Orchestrator::run(skill, inputs) -> Result<SkillOutput>`
  - `src/agent/prompt.rs` — system prompt assembly (always-loaded context placeholder for #0011)
  - `src/agent/log.rs` — per-invocation log writer (`Info/agent/invocations/<ulid>.json`)
- Existing `src/llm/` keeps Ollama transport; `agent::orchestrator` calls into it. No coach pipeline rewrite in this ticket — they continue using the direct path. #0010 onward use `Orchestrator::run`.

### Tool trait
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;        // one line, ≤120 chars (lint-enforced)
    fn permission(&self) -> Permission;
    fn schema(&self) -> ToolSchema;               // JSON schema of input
    fn call(&self, ctx: &ToolCtx, input: serde_json::Value) -> Result<serde_json::Value>;
}

pub enum Permission {
    ReadManuscript,
    ReadAuthor,
    WriteAgent,
    ProposeAuthor,        // only path that can mutate Author Layer
    Retrieve,
}
```
- `ToolCtx` carries `&Book`, mutable Agent Layer handle, and the active `InvocationId` for logging.
- **Hard rule, enforced in the orchestrator:** there is no tool with permission `WriteAuthor`. The only path to mutate the Author Layer is `author.propose` (permission `ProposeAuthor`), then explicit author confirmation via the proposal queue from #0008.

### Initial tool registry
Implement these tools (thin wrappers over #0008 accessors and Agent Layer storage):
- `manuscript.read` — `{paragraph_ids: [String]}` → `{paragraphs: [{id, text}]}`
- `manuscript.search` — `{query, limit}` → `{hits: [{paragraph_id, snippet}]}` (substring search; full-text in a later ticket)
- `characters.get` / `characters.list` / `characters.search` / `characters.mentions`
- `locations.*`, `factions.*`, `items.*`, `magic_rules.*` — same shape
- `timeline.get` / `timeline.range` / `timeline.around`
- `agent.note` — `{topic, body, refs}` → `{observation_id}`
- `agent.recall` — `{topic | entity_id}` → `{artifacts: [...]}`
- `author.propose` — `{kind, target, change, reason}` → `{proposal_id}`

`agent.summarize`, `agent.compact`, `retrieve.semantic`, `retrieve.entity_mentions` are stubbed but not registered until their backing skills/infrastructure exist (#0011 for summarize/compact, #0012 for semantic).

Each tool's `description()` is the model-facing one-liner. A unit test (`tools::tests::descriptions_under_limit`) asserts every registered tool's description is ≤120 chars; this is the enforcement of the §8 "Tool description overhead" risk in the architecture doc. **Error-level**, not warning — per the project's quality-gates rules.

### Skill struct
```rust
pub struct Skill {
    pub name: &'static str,                  // e.g. "scene_transcode"
    pub version: u32,                        // bumped on prompt or output-schema change
    pub description: &'static str,
    pub model: ModelChoice,                  // gemma4 default; per-skill override
    pub tools: &'static [&'static str],      // tool names this skill is allowed to call
    pub inputs:  SkillInputSchema,           // typed slots
    pub outputs: SkillOutputSchema,          // typed artifact shape
    pub system_prompt: SystemPromptTemplate, // renders with {always_loaded_ctx, inputs}
    pub max_tool_calls: u32,                 // hard cap, default 16
    pub adapter: Option<AdapterRef>,         // optional LoRA adapter; stub here, loaded by the deferred training-pipeline ticket
}

/// Reference to a trained LoRA adapter. Per `docs/memory-architecture.md` §6,
/// voice / recognition / aesthetic judgment belong in weights rather than context.
/// Stub in this ticket: the field exists on `Skill`, all initial skills declare
/// `adapter: None`, and the orchestrator does **not** consult it. Adapter loading
/// and the training pipeline ship in a separate, deferred ticket.
pub struct AdapterRef {
    pub name: String,                        // e.g. "voice_v3"
    pub path: PathBuf,                       // GGUF file or Ollama tag form
    pub trained_on_corpus_hash: Hash,        // captured corrections + golden held-out
    pub regression_score: f32,               // gate score against the held-out
    pub created_at: i64,
}
```
- `SkillInputSchema` and `SkillOutputSchema` are typed enums of the artifact kinds defined in #0008 (`SceneRecord`, `ChapterRollup`, `Observation`, `Summary`).
- `SkillRegistry` holds all known skills; #0010 and #0011 add their own.
- The skill struct is `'static` data — registered at startup via an inventory pattern or explicit `register_skill` in `agent::skills::all()`.
- The `adapter` field is declared now so the framework can ship without depending on the (still-deferred) training-pipeline ticket, and so #0010, #0011, #0013 don't have to retrofit the field across every registered skill later. See "Design notes" for the rationale.

### Orchestrator
```rust
pub struct Orchestrator<'a> {
    book: &'a mut Book,
    llm:  &'a llm::Client,
}

impl<'a> Orchestrator<'a> {
    pub fn run(&mut self, skill: &Skill, inputs: SkillInputs) -> Result<SkillOutput>;
}
```
The `run` method:
1. **Validate inputs** against `skill.inputs`. Reject early on shape mismatch.
2. **Build the prompt**: system prompt template rendered with always-loaded context (a placeholder bundle in #0009 — full implementation in #0011) + the skill's inputs.
3. **Build the tool subset**: only the tools named in `skill.tools`, with their permission tags. Permission `WriteAuthor` is unreachable by construction (no such tool exists).
4. **Call Ollama** with tool-use loop:
   - Send prompt + tool schemas.
   - On tool call, route through `ToolCtx` (logged), feed result back, continue.
   - Cap at `skill.max_tool_calls`; on cap, abort with `OrchestrationError::ToolCallLimit`.
5. **Parse the model's final output** against `skill.outputs`. Reject on shape mismatch with a clear error referencing the offending field.
6. **Persist**: route the typed output to the appropriate Agent Layer writer (e.g. `agent_layer::save_scene_record`).
7. **Log**: write the invocation log (see below).
8. Return the typed `SkillOutput` to the caller.

Failure modes that the orchestrator must distinguish (so caller can decide retry policy):
- `OrchestrationError::ModelUnavailable` (Ollama down / model missing)
- `OrchestrationError::ToolCallLimit`
- `OrchestrationError::OutputSchema { field, reason }`
- `OrchestrationError::ToolError { name, source }`
- `OrchestrationError::Timeout`

### Invocation log
- Path: `Info/agent/invocations/<ulid>.json`
- Shape:
  ```rust
  pub struct InvocationLog {
      pub id: String,
      pub skill: String,
      pub skill_version: u32,
      pub model: String,
      pub started_at: i64,
      pub finished_at: i64,
      pub prompt_chars: usize,
      pub prompt_tokens_est: usize,        // rough; from Ollama response if available
      pub output_tokens_est: usize,
      pub tool_calls: Vec<ToolCallLog>,    // {name, input_chars, output_chars, ms, ok}
      pub outcome: Outcome,                // Ok | Err(reason)
  }
  ```
- Daily rotation: a `compact_invocation_logs` helper that, after N days (default 30), summarises older logs into a single per-day rollup. Stub the helper here; wire to a maintenance call later.

### Always-loaded context placeholder
- `prompt::always_loaded_context(book)` returns a placeholder string that includes only the chapter title + entity counts in v1. #0011 replaces the body of this function with the real entity bible + chapter rollups.
- This placeholder lets #0010 land before #0011 without a no-op layer.

### No coach-pipeline migration
The four existing pipelines stay on their direct path. Migrating them onto `Orchestrator::run` is a separate ticket (rationale: each pipeline needs its own skill definition + careful regression testing of dismissal/lifecycle behaviour, which is too much for this scaffolding ticket).

## Out of scope
- Migrating the four existing coach pipelines onto the orchestrator.
- Concrete skills (`scene_transcode`, `chapter_rollup`, etc.) — those are #0010, #0011, and follow-ups.
- Inspector UI for invocation logs.
- Streaming output during tool-use loops.
- Multi-turn conversational agents (the orchestrator is one-shot per skill in v1; conversational coaching is a different runtime).
- Permission `WriteAuthor` — deliberately omitted; mutating Author Layer is exclusively `author.propose` + author confirmation (#0008).
- Vector-search tools (`retrieve.semantic`) — registered in #0012.
- LoRA adapter loading and training pipeline. `Skill.adapter` and `AdapterRef` exist in this ticket as a forward-compatibility stub; the orchestrator must not consult the field in v1, and no public API for loading adapters is added. End-to-end training (corpus assembly, QLoRA, regression gate, GGUF export, Ollama registration) ships in a separate, deferred training-pipeline ticket (not yet filed; see `docs/memory-architecture.md` §6.4, §7.4, and §9 for trigger conditions).
- Capturing `FeedbackRecord`s — that's part of the training-pipeline ticket (and the user-visible skills #0013+ that produce artifacts to capture corrections from).

## Acceptance criteria
- [ ] `Tool` trait exists with `name`, `description`, `permission`, `schema`, `call`.
- [ ] All initial tools (§"Initial tool registry") are registered; each tool's `description` is ≤120 chars (asserted by a `#[test]` that fails the build on regression — error, not warning).
- [ ] No registered tool has permission `WriteAuthor`. Asserted by a `#[test]` that iterates the registry.
- [ ] `Skill` struct + `SkillRegistry` exist; an empty registry compiles (skills land in #0010+).
- [ ] `Skill` struct has an `adapter: Option<AdapterRef>` field; `AdapterRef` is defined with the fields named in §"Skill struct". A `#[test]` (`skills::tests::all_initial_adapters_none`) iterates the registry and asserts every registered skill has `adapter: None` — error-level, fails the build on regression. The orchestrator must not read the field in v1; an `assert!(skill.adapter.is_none())` at the top of `Orchestrator::run` documents and enforces this until the training-pipeline ticket lifts it.
- [ ] `Orchestrator::run` happy-path test using a fake `llm::Client` + a fake skill: feeds inputs, model "calls" `characters.get`, model emits a typed output, orchestrator persists it via a fake Agent Layer writer, returns the output.
- [ ] `Orchestrator::run` rejects shape-mismatched output with `OrchestrationError::OutputSchema { field, reason }`.
- [ ] `Orchestrator::run` aborts at `max_tool_calls` with `OrchestrationError::ToolCallLimit` (test with a fake model that loops).
- [ ] `Orchestrator::run` writes an `InvocationLog` for every invocation, success or failure.
- [ ] A skill that declares an unregistered tool name fails at registration time (not at run time) — `SkillRegistry::register` returns `Err`.
- [ ] `prompt::always_loaded_context` returns a placeholder string; the function signature is the contract #0011 will fill in.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why one-shot skills, not conversational agents?** Conversational agents amplify both context cost and trust risk. The skill model — typed inputs, declared tools, typed outputs — is auditable. A coaching chat surface can sit on top later, calling skills as primitives.
- **Why JSON-schema for tool I/O instead of native Rust serde?** The schema is what gets sent to the model in tool-use mode. Generating it from Rust types (e.g. `schemars`) keeps the contract single-sourced; pull `schemars` in for this. If `schemars` proves heavy, hand-write schemas alongside types.
- **Tool description length lint.** Per `docs/memory-architecture.md` §8 "Tool description overhead": every tool registered shows up in every prompt. 20 tools × 200 chars = 4000 chars of prompt overhead per call. Cap at 120; enforce as test.
- **Trust boundary enforcement is two-layer.** Layer 1: no `WriteAuthor` permission exists in the enum's accepted-by-tools set. Layer 2: tools' `call` methods only receive `&Book` (immutable for Author entities) — the `&mut Book` lives on the orchestrator, used for Agent Layer writes only. Compile-time enforcement.
- **Why log token counts as estimates?** Ollama returns `eval_count` / `prompt_eval_count` for some models; treat as authoritative when present, fall back to char/4 heuristic when absent. Mark which is which in the log so we don't average estimates with measurements.
- **Why a separate `agent::prompt` module?** #0011 will replace `always_loaded_context` with a real builder that walks Author + Agent Layers. Keeping it isolated means #0011 is a self-contained body swap.
- **Why ulid (or similar) for invocation IDs, not uuid?** Sortable by creation time; useful when scanning the log directory.
- **Why a stub `adapter` field instead of adding it later?** Per `docs/memory-architecture.md` §6, voice and aesthetic judgment are a separate channel into the model, complementary to context. Retrofitting an adapter field across every registered skill once #0010, #0011, #0013, and follow-on coaching skills are in place would mean editing every skill registration plus the orchestrator's invocation path. Declaring the slot now, even unused, is cheap and matches the architecture-as-contract rule from `~/.claude/CLAUDE.md`. The hard rule (`assert!(skill.adapter.is_none())` in v1, plus the registry-iterating test) is the gate that prevents the field from being silently consumed before the deferred training-pipeline ticket wires it up properly.
