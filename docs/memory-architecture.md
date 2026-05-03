# Memory & agent architecture

**Status:** design — infrastructure tracked in tickets #0008–#0013; training pipeline deferred (separate ticket, not yet filed — see §9 for trigger conditions).
**Audience:** anyone (human or AI) extending CkWriter's coach/agent surface.

## 1. Why this exists

CkWriter's coach pipelines today re-prompt the local model (gemma4 via Ollama, ~124k context) with whole chapters of prose. That works for paragraph-level grammar/voice flags, but it falls apart for the things a discovery writer most needs help with:

- *Continuity across the whole book* — "you said Aren has grey eyes in ch.3, blue in ch.7"
- *Voice drift* — "the protagonist's dialogue tone shifted around ch.7"
- *Foreshadowing tracking* — "the silver coin was planted in ch.3; has it paid off?"
- *Author-intent alignment* — "the chapter says X happened, but your notes say you meant Y"

These need the agent to reason over the **whole book**, not just one chapter. Naively stuffing the whole manuscript into context is both impossible (a 120k-word novel ≈ 160k tokens of prose) and counterproductive (model attention quality collapses past a certain density of irrelevant text).

The fix is not "buy more context." It is to change *what we load*, and to recognise that not every kind of knowledge belongs in context at all. Four principles drive the rest of this document:

1. **Transcode prose, don't summarize it.** Summaries lose information; structured records preserve it. A 5000-word scene compresses to ~100 tokens of `SceneRecord` without losing the facts the agent needs to reason.
2. **Separate what the author wrote, what the author meant, and what the agent inferred.** Three storage layers, with a strict trust boundary between them. The agent must never confuse hypothesis with canon.
3. **Make the agent toolset typed and minimal.** `characters.get(id)` is better than `search("character info about X")`. Typed accessors give the agent sharp, cacheable, verifiable operations.
4. **Match each kind of knowledge to its substrate.** Facts that change per-scene and must be exact (canon entities, paragraph-level edits) live in *context*, retrieved fresh on every call. Style and recognition that change slowly and resist articulation (the author's voice, the cast of named entities, "what counts as a good passage") live in *weights*, encoded via small LoRA adapters trained from captured corrections. Hypotheses live in flagged *Agent Layer* storage. Putting facts in weights produces hallucination; putting style in prompts produces bloat. See §6.

## 2. The three layers

### 2.1 Manuscript

What's on the page. The `.tex` files plus the paragraph index from #0002.

- **Source of truth for** the text itself.
- **Written by** the author (and only the author).
- **Read by** every agent and skill.
- **Identity** at paragraph granularity via `ParagraphMeta { id, hash }`.

The Manuscript is the only layer where prose lives. Other layers reference back into it via `paragraph_id` so any retrieval can fall back to source on request.

### 2.2 Author Layer

What the author *believes/intends* about the book — canon. Today this is the `Entities` store (`Info/Characters/`, `Info/Locations/`, etc.) plus chapter metadata (#0001). The new design extends it with:

- **Typed entity kinds**: `Character`, `Location`, `Faction`, `Item`, `MagicRule`, `TimelineEvent` — and author-extensible kinds for discovery-writer surprises.
- **Per-kind schema**: each kind has its own fields, validated. `Character.eyes`, `Location.climate`, etc.
- **Author intent annotations**: optional notes on scenes/paragraphs explaining what the author meant ("this is foreshadowing X").
- **Mentions index**: every entity carries a `mentions: [paragraph_id]` list, populated by the entity-extraction skill, edited freely by the author.

**Trust:** Author Layer is **canon**. The agent reads it but does not write to it directly. Changes proposed by the agent go through an explicit `promote` flow (§4.4).

### 2.3 Agent Layer

What the agent has *figured out*. Hypotheses, derived summaries, retrieval traces, scene transcodings, observations the author hasn't been told about yet. Lives at `Info/agent/` (one sidecar per kind of artifact).

Artifact kinds (initial set; extensible):

- `SceneRecord` — structured transcoding of a scene's prose
- `ChapterRollup` — one-paragraph précis per chapter, derived from `SceneRecord`s
- `EntityObservation` — "I noticed Aren's tone shifted at scene N" — pending author review
- `RetrievalTrace` — what the agent looked up to answer Q, so it can short-circuit next time
- `Summary` — agent-built summary of an arbitrary span, cached for re-use

Every Agent Layer artifact carries:
- `source_hash` — content hash of the Manuscript region(s) it was derived from
- `source_refs` — `[paragraph_id]` covering its scope
- `created_at`, `created_by_skill`, `model_id`
- `confidence` — coarse self-rating from the producing skill (low/med/high)

**Trust:** Agent Layer is **inferred**. Every coach surface that displays an Agent Layer artifact must visibly distinguish it from Author Layer canon ("you wrote..." vs "I noticed...").

## 3. Typed entity accessors

The Author Layer presents a **typed accessor API** to skills and the inspector UI. This is what the agent calls; the underlying store can evolve without breaking callers.

```
characters.get(id) -> Character
characters.list() / characters.search(query)
characters.mentions(id) -> Vec<ParagraphRef>

locations.get(id) -> Location
locations.list() / locations.search(query)

factions.get(id) -> Faction        // new in #0008
items.get(id) -> Item              // new in #0008
magic_rules.list() -> Vec<MagicRule>
timeline.get(event_id) -> TimelineEvent
timeline.range(from, to) -> Vec<TimelineEvent>
timeline.around(event_id) -> Vec<TimelineEvent>   // events ±N around a beat
```

Accessor rules:
- **Read-only from the agent's side.** Mutations route through `author.propose(change)` (§4.4).
- **Cheap.** No model calls, no embeddings — just typed lookups against the in-memory store.
- **Stable signatures.** Adding a kind extends the accessor surface; the existing accessors do not change shape. This is the agent's API contract.

## 4. The agentic framework

### 4.1 Tools

A **Tool** is a single typed operation the agent can call. Tools are the only way an agent touches the storage layers. The initial registry:

```
manuscript.read(paragraph_id | range)
manuscript.search(query, filters)

characters.* / locations.* / factions.* / items.* / magic_rules.* / timeline.*   (§3)

agent.note(topic, body, refs) -> ObservationId
agent.recall(topic | entity_id) -> Vec<Artifact>
agent.summarize(span) -> Summary           // calls a sub-skill
agent.compact(older_than)                  // self-managed eviction

retrieve.semantic(query, scope, k)         // #0012
retrieve.entity_mentions(entity_id, scope)

author.propose(change) -> ProposalId       // §4.4
```

Each tool has:
- A short, model-facing description (one line, costs context tokens — keep it tight)
- A typed input/output schema
- A permission tag (`read_manuscript`, `read_author`, `write_agent`, `propose_author`)

### 4.2 Skills

A **Skill** is a declarative bundle of tools + I/O + a system prompt template, optionally pinned to a LoRA adapter that encodes voice/recognition/aesthetic judgment in weights. Skills are how agents do composite work.

```
Skill {
  name:        "scene_transcode",
  description: "Convert a scene's prose into a structured SceneRecord.",
  inputs:      [chapter_id, scene_range],          // typed slots
  tools:       [manuscript.read, characters.list, locations.list],
  outputs:     [agent.write(SceneRecord)],
  prompt:      <system prompt template>,
  model:       "gemma4",                            // can vary per skill
  adapter:     None,                                // Option<AdapterRef>; see §6.4
}
```

Initial skills:
- `scene_transcode` (#0010) — prose → `SceneRecord`
- `chapter_rollup` (#0011) — `SceneRecord`s → `ChapterRollup`
- `entity_extract` — find new mentions of canon entities; propose new ones
- `continuity_check` — read Author Layer + recent `SceneRecord`s, flag contradictions
- `voice_drift` — compare scene-level voice fingerprints to baseline
- `thread_tracker` — surface open `threads_opened` without matching `threads_closed`

The skill registry is what the orchestrator picks from. Skills compose: the orchestrator can pipeline `transcode → extract → check` because each one's outputs are typed.

### 4.3 Orchestrator

The orchestrator is the runtime that:
1. Picks a skill (by user trigger, on-save hook, or a parent skill's request).
2. Loads the skill's declared inputs.
3. Renders the system prompt with the always-loaded context (§5.1) plus the skill's inputs.
4. Calls the local model with the skill's allowed tool subset.
5. Validates outputs against the declared schema.
6. Persists outputs to the Agent Layer.
7. Logs invocation, tool calls, token counts, latency.

Skills do not write to disk directly. The orchestrator does, after validation.

### 4.4 Promotion: Agent → Author

The agent must **never** write directly to the Author Layer. When `entity_extract` finds a probably-new character, or `continuity_check` notices the author has implicitly settled on "blue eyes," the skill emits an `author.propose(change)` instead of a write.

Proposals land in a queue surfaced in the inspector UI. The author confirms, edits, or rejects. Confirmed proposals become canon; rejected ones are recorded in the Agent Layer so the same proposal isn't re-emitted.

This is the single trust valve between layers. Without it, the agent's hallucinations would silently pollute the bible.

## 5. Compression strategy

### 5.1 Always-loaded context

Every coach/skill invocation loads, at minimum:

| Slice | Source | Approx. tokens |
|---|---|---|
| Entity bible | Author Layer, all kinds, compact form | ~10k |
| Chapter rollups | Agent Layer `ChapterRollup` × N chapters | ~6k |
| Current chapter prose | Manuscript | ~5k |
| **Total baseline** | | **~21k** |

That leaves ~100k headroom in gemma4's 124k window for the active conversation, retrieved scene records, and tool dialogue.

The entity bible is rebuilt from the Author Layer on demand and cached until any Author Layer file changes. It uses a compact rendering (one line per entity attribute, omitting empty fields), not the full JSON.

### 5.2 On-demand retrieval

When a skill or agent needs more, it pulls via tools:

1. **Structured retrieval first** — `characters.mentions(id)`, `timeline.around(event)`, `retrieve.entity_mentions`. Sharp, deterministic, cheap.
2. **Vector retrieval as fallback** — `retrieve.semantic(query, scope)` (#0012). Used when the question is fuzzy ("scenes with grief tones") and structured filters can't pin it down.

Structured retrieval should handle ~80% of needs. Vector search is the long tail.

### 5.3 Transcoding (the core compression trick)

`scene_transcode` (#0010) turns each scene's prose into a `SceneRecord`:

```rust
struct SceneRecord {
    id: SceneId,
    chapter: ChapterId,
    paragraph_range: (ParagraphId, ParagraphId),
    pov: Option<EntityId>,
    present: Vec<EntityId>,
    location: Option<EntityId>,
    beats: Vec<Event>,             // {actor, verb, object, modifiers}
    entity_deltas: Vec<Delta>,     // what changed about each entity
    threads_opened: Vec<ThreadId>, // foreshadowing planted
    threads_closed: Vec<ThreadId>, // payoff landed
    author_intent: Option<String>, // copied from Author Layer if present
    source_hash: Hash,
    source_refs: Vec<ParagraphId>,
    confidence: Confidence,
}
```

A typical novel of 500 scenes × ~100 tokens per record ≈ 50k tokens for the **entire book in structured form**. That fits comfortably in context, and unlike prose summaries, the agent can reason over it field-by-field.

When the agent needs the actual prose for a record (e.g., to quote it back to the author), it calls `manuscript.read(record.source_refs)`.

### 5.4 Staleness & invalidation

Every Agent Layer artifact carries `source_hash`. When a paragraph's hash changes (paragraph index from #0002 detects this on save), every Agent Layer artifact whose `source_refs` includes that paragraph is marked stale. Stale artifacts are:

- Not used by skills (skills request fresh ones via re-derivation)
- Visible in the inspector with a "stale" badge
- Re-derived lazily on next request, not eagerly on edit (avoids thrashing during active writing)

### 5.5 Compaction

The Agent Layer can grow unbounded if left alone (every `EntityObservation`, every `RetrievalTrace`). `agent.compact(older_than)` summarizes old artifacts of the same kind/topic into a single condensed artifact, dropping the originals. This is itself a skill that the orchestrator can run periodically.

## 6. What goes in weights vs context

CkWriter combines two complementary mechanisms for putting knowledge in front of the model: **context injection** (prompt-time facts retrieved fresh from storage, §5) and **weight encoding** (small LoRA adapters trained on captured author corrections). They are not interchangeable. Getting the split wrong is the most common failure mode for AI writing tools — and the one that compounds silently.

### 6.1 The substrate matrix

| Update frequency | Verifiability | Substrate | Examples |
|---|---|---|---|
| High (per scene) | Must be exact | **Context** — Author Layer, retrieval | Aren's eye color; chapter 14's events; the current entity bible |
| Low (per arc) | Ineffable, aesthetic | **Weights** — LoRA adapter | The author's voice; recognition of canon entities; "what counts as a good passage" |
| High, speculative | Must be flagged as inferred | **Agent Layer** — flagged storage | Continuity hypotheses; transcoded scene records; agent observations |

The matrix is the heuristic. Each kind of knowledge has a natural update rhythm and a verifiability profile, and those two dimensions point at the substrate that fits.

### 6.2 What belongs in weights

Three things, sharply distinguished:

- **Recognition.** Training reliably teaches the model that "Aren," "Elenya," and "the silver coin" are real entities in this world. This makes downstream skills sharper: better entity extraction, better dialogue continuation, fewer "did you mean...?" misreads on rare names. Recognition does **not** mean memorising facts about those entities.
- **Voice and style.** Sentence rhythms, dialogue conventions, scene shape, the cadence of a paragraph break. These are hard to articulate as prompt rules and slow to change. A LoRA adapter trained on the manuscript itself, plus author-marked "good passage" exemplars, internalises them.
- **Aesthetic judgment.** "What counts as a good passage *in this book*." Exemplars labelled by the author teach the model to score and rewrite in the author's preferred direction without that direction having to be enumerated.

### 6.3 What does NOT belong in weights

- **Facts about entities.** Training will encode that Aren exists; it will *not* reliably encode that Aren has grey eyes. Fine-tuned facts hallucinate confidently — the model recalls the name and confabulates the attribute. Facts go in the Author Layer; the entity bible is the canonical answer.
- **Plot state.** What happened in ch.14, who knows what at scene N, the timeline as it currently stands. These change as the manuscript changes; a weekly retrain will lag the manuscript by definition.
- **Anything you must be able to correct in seconds.** Weights are batched; context is live. A character rename, a continuity fix, a chapter reordering must take effect on the next inference, not the next adapter release.

If facts leak into weights they become un-correctable without retraining. If style leaks into prompts it bloats context and never quite captures what the author means. The membrane between weights and context is as load-bearing as the membrane between Author and Agent layers.

### 6.4 LoRA adapters per skill

Each `Skill` (§4.2) carries an optional adapter:

```rust
pub struct Skill {
    // ... fields from §4.2 ...
    pub adapter: Option<AdapterRef>,
}

pub struct AdapterRef {
    pub name: String,                      // e.g. "voice_v3", "good_passage_v2"
    pub path: PathBuf,                     // GGUF file or Ollama tag
    pub trained_on_corpus_hash: Hash,      // captured corrections + golden held-out
    pub regression_score: f32,             // gate score against the golden held-out
    pub created_at: i64,
}
```

The orchestrator (§4.3) loads the adapter when invoking the model for a skill that declares one. Adapters are local-trainable on consumer hardware — Gemma 4 E2B fine-tunes on ~8–10 GB VRAM via Unsloth + 4-bit QLoRA, producing hundreds of MB of weights on top of the shared base model. The base is shared across all skills; adapters are per-skill and hot-swappable.

The training pipeline, cadence, and corpus assembly are §7. The adapter slot is declared in #0009 as a stub field (`Option<AdapterRef>`, defaulting to `None`); orchestrator-side loading and the training pipeline ship in a separate, deferred training-pipeline ticket (see §9).

### 6.5 The combination

A skill at runtime is the assembly of all five inputs, each carrying a different kind of knowledge:

1. The **shared base model** — general language ability.
2. The skill's **adapter**, if any — voice, recognition, aesthetic judgment.
3. **Always-loaded context** (§5.1) — entity bible, chapter rollups, current chapter prose.
4. **Skill-specific retrieved records** — typed lookups via the tool registry.
5. **Per-skill exemplars** (§7.3) — corrected (input, output) pairs from captured feedback.

Each input has a different update rhythm and a different correction path. The architecture is the *combination*; either mechanism alone produces a recognisable failure mode (prompt-only is bloated and voice-blind; weights-only is hallucinatory and stale).

## 7. Human-in-the-loop feedback

Both substrates — context and weights — improve with the same input: corrections the author has already produced. This section describes how those corrections are captured and routed.

### 7.1 Two paths

| Path | Latency | Mechanism | Used for |
|---|---|---|---|
| **Hot** | Next inference | Exemplars + dictionary overrides injected into context | Long-tail corrections; per-author quirks; skill-specific suppressions |
| **Cold** | Next adapter release | Training corpus for LoRA retrain | Voice, recognition, aesthetic judgment |

Every captured signal feeds the hot path immediately and accumulates toward the cold path for the next batched retrain. One capture, two consumers.

### 7.2 Implicit signals

Capture-without-asking is the load-bearing channel. Explicit feedback prompts erode quickly under repeated use; implicit signals scale across years of writing.

- **Edits as gold.** When the author corrects an agent-produced artifact (a `SceneRecord` field, a continuity flag, a paragraph rewrite suggestion) the before/after pair is recorded. Highest-quality signal in the system.
- **Promotions as positive.** A `promote(agent_note → author_entry)` (§4.4) is the author affirming "this hypothesis was correct enough to enter canon."
- **Dismissals as negative.** Reusing the persistent dismissal filter pattern from #0003: a flagged issue dismissed *N* times in similar contexts becomes a per-skill suppression rule.
- **Re-runs as latent failure.** If a skill is invoked twice on the same scope without intervening edits, the first run was unsatisfying. Tracked per-skill as a soft regression signal.

Each signal lands in `Info/agent/feedback/<skill>/<ulid>.json` with a stable schema:

```rust
pub struct FeedbackRecord {
    pub id: String,
    pub skill: String,
    pub skill_version: u32,
    pub adapter_version: Option<String>,   // which adapter was active when produced
    pub kind: FeedbackKind,                // Edit | Promote | Dismiss | Rerun
    pub source_artifact: ArtifactRef,
    pub before: Option<serde_json::Value>,
    pub after:  Option<serde_json::Value>,
    pub note:   Option<String>,            // optional one-line author comment
    pub captured_at: i64,
}
```

Schema is shared across hot and cold paths.

### 7.3 Hot path: exemplars + overrides

Three uses, all context-engineering:

- **Per-skill exemplar bank.** Each skill curates a bank of (input, output) pairs from captured `Edit` records. At inference, the orchestrator retrieves the *N* exemplars most relevant to the current input (by entity, chapter range, or vector similarity from #0012) and injects them into the system prompt. The seed for the transcoder skill is the hand-checked fixture from #0010 (`tests/data/transcode/awakening/`); other skills seed from their first batches of captured edits.
- **Per-author dictionary / canon overrides.** "Aren has grey eyes (you've corrected this twice)" becomes a hard constraint injected for any skill touching that entity. Compresses repeated correction loops to zero.
- **Negative-example filter.** Patterns dismissed *N* times stop being surfaced by the producing skill. Generalises the existing dismissal filter.

### 7.4 Cold path: adapter retraining

Triggered manually or by a per-skill data threshold (e.g. "voice adapter has 100+ new edit pairs since last retrain"). The pipeline (a separate, deferred ticket — see §9):

1. **Assemble corpus.** Convert relevant `FeedbackRecord`s for the target skill into training examples. Hold out a regression set: the seed golden corpus for the skill plus the most recent *N*% of captures.
2. **Train.** Local Unsloth + QLoRA on the chosen base model (gemma4 E2B/E4B). Documented defaults for small datasets: LR ~2e-4, ~3 epochs, rank 16.
3. **Regression gate.** Run the candidate adapter against the held-out set. Fail the swap if recall, precision, or task-specific scores degrade past a per-skill threshold. Per the project's ratchet rule (`~/.claude/CLAUDE.md`), the threshold tightens on every passing release.
4. **Export & register.** GGUF export, Ollama tag, register the new `AdapterRef` in the skill's adapter slot.
5. **Bisectable swap.** Old version preserved for rollback. Skill invocation logs (§4.3) record the active adapter version, so a regression spotted later can be traced to the responsible adapter.

Cadence is per-skill, not uniform:

- **Voice / style adapter** — slow cadence (per-arc, per-N-chapters). Voice changes slowly; over-retraining chases noise.
- **Structural skills** (transcoder, rollup, continuity_check) — rarely retrained. Schema and behaviour are stable; context provides the changing facts. Retrain only on schema or prompt-template change.
- **Preference / "good passage" adapter** — retrain when ~50+ new labels accumulate.

Adapters are **releases, not streams.** Training is an authored event, not a continuous loop.

### 7.5 Reward-hacking guardrail

The single most important risk in this loop: if a skill is rewarded only by acceptance, it learns to surface only safe observations. A coaching tool that stops challenging the author has failed silently — the worst kind of failure for a tool whose value is honest pushback.

Mitigation is to track two rates per skill, not one:

- **Surface rate** — how often the skill produces an artifact when invoked.
- **Acceptance rate** — how often produced artifacts are accepted (not edited or dismissed).

A skill whose surface rate drops sharply is as broken as one whose acceptance rate drops. The recalibration job (§7.6) trips on either signal.

### 7.6 Recalibration

A weekly background job computes per-skill surface and acceptance rates over the trailing window and surfaces a notification when either deviates from a baseline:

> `voice_drift`: surface rate 4% (baseline 22%), acceptance 91%. Likely stuck — review.

The author decides the response (review the skill prompt, retrain the adapter, suppress the notification, retire the skill). The system never auto-retrains without explicit consent.

### 7.7 Staleness in feedback itself

Author voice changes across a multi-year project. Captured corrections from ch.3 may not represent the author's current preferences by ch.20. Each `FeedbackRecord` carries `captured_at`; the cold-path corpus assembly applies a decay weighting (newer records weighted higher) and the author can mark records "stale" to exclude them. The hot path uses the same decay when ranking exemplars.

## 8. Risks & open questions

**Schema lock-in.** Discovery writers invent new entity kinds mid-book. The accessor API (§3) is extensible; adding a kind extends the accessor surface but does not break existing accessors. New kinds register via a `register_entity_kind` call, with a JSON schema for fields. Author can edit field schemas; the agent cannot.

**Trust leakage.** The biggest failure mode is the agent treating its own observations as canon on a later run. Mitigations: (a) Author Layer artifacts and Agent Layer artifacts are loaded into the prompt with distinct headers; (b) the system prompt is explicit about which is which; (c) every UI surface that shows agent inferences labels them as such.

**Transcoding fidelity.** The transcoder will miss things or get them wrong. Mitigations: (a) every `SceneRecord` carries `source_refs` so the agent can fall back to prose; (b) low-`confidence` records are flagged in the inspector; (c) the author can edit a `SceneRecord`'s fields manually, which pins the record (a pinned record is not invalidated by source-hash changes — the author's curation overrides re-derivation).

**Tool description overhead.** Listing 20+ tools to the model costs context every call. Mitigations: per-skill tool subsets (a skill only sees the tools it declared), and one-line descriptions enforced as a length lint.

**Concurrency.** The author edits the manuscript while the orchestrator is transcoding. Stable paragraph IDs (#0002) make this consistent: a transcoding job operates on a specific `source_hash`; if the hash has moved on by save time, the resulting artifact is born stale and re-queued.

**Vector retrieval is last.** #0012 builds local embeddings. Don't build it until #0008–#0011 are in use and we have evidence that structured retrieval doesn't cover the cases users hit. It is genuinely possible we never need it.

## 9. Implementation sequencing

The five infrastructure tickets (#0008–#0012) plus the first user-visible consumer (#0013) form the bundle. The dependency graph:

```
  #0008 storage + typed accessors + Agent Layer
    │
    ├── #0009 skill + tool framework + orchestrator
    │     │
    │     ├── #0010 scene_transcode skill + SceneRecord ──┐
    │     ├── #0011 always-loaded context (bible+rollup) ─┤
    │     └── #0012 vector retrieval (semantic fallback) ─┤
    │                                                     ▼
    │                                          #0013 continuity_check skill
    │                                          (first user-visible consumer)
    │
    └── (#0012 also depends only on #0008 directly)
```

**Strict order:**
- #0009 needs #0008 (the typed accessors are what makes the tool surface meaningful).
- #0010, #0011, #0013 need #0008 and #0009.
- #0012 needs #0008; uses #0009's tool registry but doesn't need #0010/#0011.

**Parallelisable** (after #0009 lands):
- #0010, #0011, #0012 are independent of each other. They can be picked up by separate sessions.
- #0013 must wait for #0010 and #0011 (it consumes their outputs); #0012 is optional for it.

**Cross-ticket relationships in the existing backlog/todo:**
- **#0006 (embedding fuzzy dedup, in `todo/`)** — overlaps with #0012's embedding primitive. Resolution noted in #0012: when #0012 lands, #0006 should be reframed to consume the same `fastembed-rs` substrate, OR closed if #0007 makes residual duplicates negligible. Don't ship two embedders.
- **#0007 (paragraph-focused coaching, in `backlog/`)** — independent of this stack. Plausibly the second consumer after #0013, but not refactored to depend on it.
- **#0004 (per-paragraph caching, in `todo/`)** — same pattern as #0010's per-scene staleness, but no shared storage. Noted in #0010's design notes; not a hard prerequisite.

**The shipping discipline:** don't pick #0010, #0011, or #0012 up speculatively. Each is justified by #0013 needing it. If #0013 gets implemented and #0012 turns out unused, that's a signal to close #0012 rather than ship infrastructure with no consumer.

**Training pipeline (deferred, not yet filed).** The cold feedback path (§7.4) — `FeedbackRecord` corpus assembly, Unsloth/QLoRA training, regression gate against the held-out golden set, GGUF export, adapter registration — is its own ticket, deferred. It depends on #0009 only structurally (the `adapter: Option<AdapterRef>` slot lands in #0009 as a stub field). It depends on #0013-and-beyond *operationally*: there is no point retraining an adapter until a skill has accumulated enough captured corrections to make a measurable difference, and that data only exists once user-visible skills are running. File the training-pipeline ticket when:

- At least one skill has accumulated ~100+ captured `Edit`/`Promote`/`Dismiss` records, **or**
- An ad-hoc voice/style retrain on the manuscript itself is wanted ahead of feedback accumulation.

Until then, the hot feedback path (§7.3) — exemplar bank + dictionary overrides + negative-example filter — does the work.

## 10. Glossary

- **Manuscript** — the `.tex` prose, paragraph-indexed.
- **Author Layer** — typed canon entities + author intent annotations. Author writes, agent reads.
- **Agent Layer** — derived artifacts (transcodings, observations, summaries, traces). Agent writes, author reads.
- **Tool** — a single typed operation an agent can call.
- **Skill** — a declarative bundle of tools + inputs/outputs + system prompt.
- **Orchestrator** — the runtime that picks skills, loads context, runs the model, persists outputs.
- **SceneRecord** — structured transcoding of a scene's prose; the unit of compression.
- **ChapterRollup** — one-paragraph précis per chapter; derived from `SceneRecord`s.
- **Always-loaded context** — entity bible + chapter rollups + current chapter prose; baseline for every skill invocation.
- **Source hash** — content hash binding an Agent Layer artifact to the Manuscript region it was derived from. Drives staleness.
- **Promotion** — explicit author confirmation that turns an agent proposal into Author Layer canon.
- **Trust boundary** — the rule that the agent never writes directly to the Author Layer; all writes go through `author.propose`.
- **Substrate** — one of the three places knowledge can live: context (retrieved fresh), weights (LoRA adapter, slow-changing), or Agent Layer storage (flagged as inferred). See §6.
- **Adapter / `AdapterRef`** — a per-skill LoRA fine-tune that encodes voice, recognition, or aesthetic judgment in weights rather than context. Optional on every skill. Local-trainable via Unsloth + QLoRA; loaded by the orchestrator at invocation.
- **Recognition vs facts** — recognition (this name is canon, this voice is mine) trains reliably into weights. Facts (Aren's eyes are grey) do not, and must come from context. The most common training failure mode is letting facts leak into weights.
- **Hot path / cold path** — the two routes feedback takes. Hot: exemplars + dictionary overrides injected into the next inference. Cold: training corpus for the next adapter retrain. Same captured signals feed both.
- **`FeedbackRecord`** — implicit signal captured from author behaviour (edit, promote, dismiss, rerun). Stored at `Info/agent/feedback/<skill>/<ulid>.json`. Consumed by both hot and cold paths.
- **Golden corpus** — held-out (input, output) pairs the author has hand-validated. Acts as the regression gate for adapter retraining; bootstraps the per-skill exemplar bank.
- **Surface rate / acceptance rate** — the two metrics tracked per skill. Both must stay healthy; a sharply dropping surface rate is as bad as a dropping acceptance rate (the reward-hacking signature).
- **Reward hacking** — the failure mode where a skill learns to surface only safe observations to maximise acceptance. Mitigated by tracking surface rate alongside acceptance rate.
