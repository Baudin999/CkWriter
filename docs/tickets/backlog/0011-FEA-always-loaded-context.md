# 0011 — FEA: Always-loaded context (entity bible + chapter rollups)

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0008, #0009
**Soft-depends on:** #0010 (chapter rollups become much more useful when scene records exist; this ticket ships a fallback rollup that works on prose summaries until then)

## Problem
Even with `SceneRecord`s available (#0010), every skill invocation needs a *baseline* slice of the book in context to reason. Today the coach pipelines build their own ad-hoc preamble (voice prompt + roadmap + cast); after #0009 the orchestrator has a `prompt::always_loaded_context` placeholder that returns nothing useful.

`docs/memory-architecture.md` §5.1 specifies the baseline:

| Slice | Source | Approx. tokens |
|---|---|---|
| Entity bible | Author Layer, all kinds, compact form | ~10k |
| Chapter rollups | Agent Layer `ChapterRollup` × N chapters | ~6k |
| Current chapter prose | Manuscript | ~5k |
| **Total baseline** | | **~21k** |

This ticket replaces the placeholder with the real implementation: a typed entity-bible builder, a `chapter_rollup` skill that produces `ChapterRollup` artifacts, and a context assembler that hands the full bundle to the orchestrator.

## Scope

### Entity bible builder
Module `src/agent/bible.rs`:
```rust
pub fn render_entity_bible(book: &Book, budget_tokens: usize) -> EntityBible;

pub struct EntityBible {
    pub text: String,                    // compact one-line-per-attribute rendering
    pub token_estimate: usize,
    pub truncated_kinds: Vec<EntityKind>, // kinds dropped or summarised under budget pressure
}
```
- Walks `book.accessors().characters().list()`, `.locations().list()`, `.factions().list()`, `.items().list()`, `.magic_rules().list()`, then a digest of `.timeline().range(...)`.
- Compact rendering: one section per kind, one line per entity, fields joined as `key: value` pairs, empty fields omitted.
- Budget enforcement: if the rendered bible exceeds `budget_tokens`, drop kinds in this priority order (least to most important): `Item`, `Faction`, `MagicRule`, `Location`, `Character`. Within a kind, drop entities by `last_seen_in_chapter` recency (oldest first). Note dropped kinds in `truncated_kinds` so the orchestrator can log it.
- `budget_tokens` default: 10_000. Configurable per skill (a skill that needs a bigger bible can pass a larger budget; a small fast skill can pass a smaller one).
- Cache: hold the rendered bible in `Book` keyed by a hash of all entity files' mtimes (cheap to compute on each call). Invalidate on entity save. Saves re-rendering on every skill call.

### ChapterRollup struct
Lives in `src/book/agent_layer.rs`:
```rust
pub struct ChapterRollup {
    pub meta: AgentArtifactMeta,
    pub chapter: String,                 // chapter `name`
    pub précis: String,                  // 1 paragraph, ~150–250 tokens
    pub key_beats: Vec<String>,          // 3–6 bullet beats (very short)
    pub characters_present: Vec<String>, // EntityIds, deduped
    pub locations: Vec<String>,
    pub threads_opened: Vec<ThreadRef>,
    pub threads_closed: Vec<ThreadRef>,
}
```
- One file per chapter: `Info/agent/chapter_rollups/<chapter>.json`.

### chapter_rollup skill
Registered in `src/agent/skills.rs`:
```rust
pub const CHAPTER_ROLLUP: Skill = Skill {
    name: "chapter_rollup",
    version: 1,
    description: "Summarise a chapter into a précis + key beats + present entities + threads.",
    model: ModelChoice::Default,
    tools: &["agent.recall", "manuscript.read", "characters.list", "locations.list"],
    inputs:  SkillInputSchema::ChapterRef,
    outputs: SkillOutputSchema::ChapterRollup,
    system_prompt: include_template!("chapter_rollup.md"),
    max_tool_calls: 8,
};
```
Two execution paths inside the skill's prompt:
- **Preferred path: SceneRecords available.** The skill fetches `Vec<SceneRecord>` for the chapter via `agent.recall(topic="scene_records", chapter=...)`. The model derives the rollup from the structured records — fast, cheap, faithful to the transcoder's output.
- **Fallback path: no SceneRecords.** If `agent.recall` returns empty, the skill reads chapter prose via `manuscript.read(...)` and derives the rollup directly from prose. Higher cost, lower fidelity, but lets #0011 ship and be useful before #0010 is rolled out across the book.

The prompt template instructs the model to prefer scene records when present and fall back to prose when not. The skill itself doesn't branch — the model decides based on what `agent.recall` returns.

### Context assembler
Replaces the placeholder body of `agent::prompt::always_loaded_context`:
```rust
pub fn always_loaded_context(book: &Book, focus: ContextFocus) -> AlwaysLoaded {
    let bible = render_entity_bible(book, focus.bible_budget);
    let rollups = collect_chapter_rollups(book, focus);    // see Selection rules
    let current = focus.current_chapter_prose(book);
    AlwaysLoaded { bible, rollups, current, total_token_estimate: ... }
}

pub struct ContextFocus {
    pub current_chapter: Option<String>, // chapter `name`
    pub bible_budget: usize,             // default 10_000
    pub rollups_budget: usize,           // default 6_000
    pub include_current_prose: bool,     // default true
}
```

Selection rules for `collect_chapter_rollups`:
1. Always include the rollup for `current_chapter` if any.
2. Include rollups for chapters in manuscript order, packed greedy until `rollups_budget` is reached.
3. If a chapter has no rollup, the assembler does **not** transparently invoke the rollup skill. It logs a `missing_rollup` warning and proceeds. Triggering the rollup skill is an explicit action (button + setting), to keep the orchestrator's behaviour predictable.
4. Stale rollups (per `is_stale`) are included with a `[stale]` marker prefixed in their rendered text, so the consuming skill can decide whether to trust them.

### Triggering chapter_rollup
- **Manual:** "Build chapter rollup" button on the Chapter tab.
- **Bulk:** "Rebuild all rollups" command in a settings menu (runs the skill across all chapters; useful after a transcoder run across the book).
- **Lazy on read:** the assembler does not auto-invoke (see selection rule 3). Adding auto-invocation is a follow-up once we have evidence the manual flow is too tedious.

### Wiring into the orchestrator
`Orchestrator::run` already calls `always_loaded_context` (placeholder). Once this ticket lands, the placeholder returns the real bundle. Skills consume it as part of their system prompt; the prompt template uses three named slots (`{{entity_bible}}`, `{{chapter_rollups}}`, `{{current_chapter}}`) so skills can choose which slots they want and skip the others.

### Coach-pipeline migration is still out of scope
The four existing coach pipelines (voice/show/prose/spelling) keep their direct path (per #0009). A separate follow-up ticket will migrate them onto `Orchestrator::run` so they automatically receive the always-loaded bundle. Scoped out of #0011 to keep the diff reviewable.

## Out of scope
- Auto-invoking `chapter_rollup` from the assembler when a rollup is missing.
- Migrating existing coach pipelines onto the orchestrator (separate follow-up).
- Inspector UI for browsing rollups or inspecting bible truncation.
- Per-character "bible card" rendering in a UI panel (storage exists, UI is a separate ticket).
- Full-text search across rollups (would help future skills; not needed here).

## Acceptance criteria
- [ ] `render_entity_bible(book, 10_000)` produces a bible whose token estimate ≤ 10_000; `truncated_kinds` lists any kinds dropped.
- [ ] Bible cache invalidates when an entity file mtime changes; verified by a test that calls twice, mutates an entity, and observes a re-render.
- [ ] Fixture book with 5 characters, 3 locations, 2 items, 0 factions, 0 magic rules renders a stable bible string (golden test).
- [ ] `ChapterRollup` round-trips through serde; staleness detected via `agent_layer::is_stale` when an underlying paragraph hash changes.
- [ ] `CHAPTER_ROLLUP` skill registered; tools resolve.
- [ ] Running the rollup skill against a fixture chapter with stub `SceneRecord`s persists a `ChapterRollup` whose `characters_present` matches the union of scene-record presents.
- [ ] Running the rollup skill against a fixture chapter with **no** scene records still produces a valid rollup (fallback path); test uses a stub LLM that exercises the prose-fallback branch.
- [ ] `always_loaded_context` returns a bundle with non-empty `bible`, the requested chapter's rollup if present, and the current chapter's prose; all within their respective token budgets.
- [ ] Missing rollups produce a `missing_rollup` warning in the invocation log; skill execution proceeds.
- [ ] Stale rollups in the assembled context carry a `[stale]` marker in their rendered text.
- [ ] "Build chapter rollup" button works on the Chapter tab.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why kind-priority dropping in the bible truncator?** Characters carry the most reasoning weight in a novel; locations next; magic rules and factions next; items last. This ordering is a default — a per-skill override can pass a different priority list when (e.g.) a magic-system skill needs `MagicRule` to outrank `Character`.
- **Why include the current chapter's prose at all?** The current chapter is the writer's working surface; coaching skills need the actual text to anchor their flags to specific quotes. Rollups + bible give the rest of the book in compressed form; the current chapter is the one slice where prose fidelity matters.
- **Why not auto-invoke `chapter_rollup` when a rollup is missing?** Implicit work inside the assembler turns "open the chapter" into "fire 30 background skill runs" the first time. Explicit invocation is predictable; we can add an opt-in setting later if the manual flow proves tedious.
- **Stale rollups are included, not dropped.** A stale rollup is more useful than no rollup — the agent can read it, weigh it ("this might be from before the recent edits"), and ask for a refresh if needed. Dropping silently would hide that something is stale.
- **Why hold the bible cache on `Book`, not on a global?** Rendering depends on entity state; tying the cache to the book object means it dies with the book on close, no global eviction policy needed.
- **Token estimation.** Use `prompt_tokens_est` from the orchestrator's heuristic (chars/4 unless a tokenizer is available). Estimates being off by 20% is fine; the budget is conservative (~21k baseline against 124k window).
- **Compact bible format.** One line per entity; field names elided when self-evident; relations expressed as `→ <id>:<kind>`. Aim for ~30 tokens per character entity, ~20 for locations.
