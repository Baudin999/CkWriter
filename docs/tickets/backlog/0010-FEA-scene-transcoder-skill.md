# 0010 — FEA: Scene transcoder skill (compression by structure)

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0008, #0009
**Related:** #0004 (shares the "don't redo work for unchanged content" pattern; not a hard prerequisite)

## Problem
A 120k-word novel is ~160k tokens of prose — too much to load into gemma4 even at 124k context, and quality of model attention degrades long before the limit is reached. Coach pipelines today work around this by only loading the current chapter, but that limits every cross-book capability we want to build (continuity checking, foreshadowing tracking, voice drift, "where else does this character appear").

`docs/memory-architecture.md` §5.3 describes the fix: **transcode prose into structured `SceneRecord`s.** A 5000-word scene compresses to ~100 tokens of typed fields without losing the facts the agent needs to reason. A whole novel of 500 scenes ≈ 50k tokens of records — fits comfortably alongside the entity bible (#0011) with room for active conversation.

This ticket ships the first concrete skill on the framework from #0009: a `scene_transcode` skill that turns scene prose into `SceneRecord` artifacts persisted to the Agent Layer.

## Scope

### Scene boundaries
A "scene" in v1 is **paragraph-range, not author-tagged**. The transcoder's first step is to pick scene boundaries inside the chapter:
- Start of chapter → first paragraph.
- A scene ends when the model emits a scene boundary in its output (typed field, see below).
- An optional author-provided boundary (LaTeX comment marker `% scene-break` on its own line) forces a boundary the transcoder must respect.
- Min scene size: 1 paragraph. Max scene size: 30 paragraphs (cap; if the model returns a scene larger than this, log a warning and split at the cap).

The transcoder runs the model **once per chapter** on the chapter's full prose, asking it to emit an array of `SceneRecord`s covering the chapter end-to-end. Per-scene model calls would multiply cost and lose cross-scene continuity context within the chapter.

### SceneRecord struct
Lives in `src/book/agent_layer.rs` (artifact added there per #0008):
```rust
pub struct SceneRecord {
    pub meta: AgentArtifactMeta,         // from #0008: schema, source_hash, source_refs, ...
    pub id: String,                      // "<chapter_name>_s<index>" — stable per chapter+ordinal
    pub chapter: String,                 // chapter `name` (the stable CamelCase id)
    pub ordinal: u32,                    // 0-based scene index within chapter
    pub paragraph_range: (String, String), // first and last paragraph_id (inclusive)
    pub pov: Option<String>,             // EntityId of POV character; None if ambiguous
    pub present: Vec<String>,            // EntityIds of characters present
    pub location: Option<String>,        // EntityId of location; None if off-screen / unspecified
    pub beats: Vec<Event>,               // ordered events
    pub entity_deltas: Vec<EntityDelta>, // what changed about each present entity
    pub threads_opened: Vec<ThreadRef>,  // foreshadowing planted
    pub threads_closed: Vec<ThreadRef>,  // payoff landed
    pub author_intent: Option<String>,   // copied from Author Layer scene-intent annotation if present
}

pub struct Event {
    pub actor: Option<String>,           // EntityId; None for environment events
    pub verb: String,                    // free-form, lowercase, ≤40 chars
    pub object: Option<String>,          // EntityId or free-form short noun
    pub modifiers: Vec<String>,          // adverbial / qualifier strings
    pub paragraph_id: String,            // anchor back into the manuscript
}

pub struct EntityDelta {
    pub entity_id: String,
    pub field: String,                   // e.g. "tone", "knows", "location", or "free:promise-broken"
    pub before: Option<String>,
    pub after: Option<String>,
    pub paragraph_id: String,
}

pub struct ThreadRef {
    pub thread_id: String,               // slug; agent invents new ids if needed (kebab-case)
    pub label: String,                   // human-readable hook
    pub paragraph_id: String,
}
```

### Skill definition
Registered in `src/agent/skills.rs`:
```rust
pub const SCENE_TRANSCODE: Skill = Skill {
    name: "scene_transcode",
    version: 1,
    description: "Compress a chapter's prose into typed SceneRecords without losing facts.",
    model: ModelChoice::Default,         // gemma4 unless overridden
    tools: &[
        "manuscript.read",               // for paragraph_id ↔ text mapping
        "characters.list",               // EntityId resolution
        "characters.search",
        "locations.list",
        "locations.search",
        "items.list",
        "factions.list",
    ],
    inputs:  SkillInputSchema::ChapterRef,    // {chapter_name}
    outputs: SkillOutputSchema::SceneRecords, // Vec<SceneRecord>
    system_prompt: include_template!("scene_transcode.md"),
    max_tool_calls: 32,                  // chapter-level work; needs more lookups
};
```
- `system_prompt` is a Markdown file under `prompts/scene_transcode.md`. It instructs the model to:
  1. Read the chapter prose (provided in the rendered prompt).
  2. Resolve character/location names to EntityIds via the typed accessors. Unrecognised names get `None` (do **not** call `author.propose` from this skill — that's the entity-extraction skill's job).
  3. Emit a JSON array of `SceneRecord` shaped exactly per the schema, with a `confidence` self-rating per scene.
  4. Anchor every `Event`, `EntityDelta`, and `ThreadRef` to a specific `paragraph_id`.
- The system prompt is part of the skill version. Bumping it bumps `Skill.version` (existing records remain valid; reads are version-agnostic, the skill version only affects newly produced records).

### Persistence
- One file per scene: `Info/agent/scene_records/<chapter>/<ordinal>.json`
- Writing the chapter's records is atomic per-chapter: the orchestrator writes to a temp directory then renames into place, replacing the old per-chapter directory's contents. This avoids partial states if a transcode is interrupted mid-write.
- Old scene records that no longer exist after re-transcoding (chapter shortened) are deleted by the rename.

### Triggering
- **Manual:** new button in the Chapter tab — "Transcode scenes". Runs the skill on the open chapter, shows progress.
- **On save:** behind a per-book setting (`auto_transcode_on_save`, default **off** in v1). When enabled, save triggers a transcode run for the just-saved chapter if and only if any of its paragraph hashes changed. Stale records elsewhere in the book are not re-transcoded eagerly — that's lazy on next read.
- **Lazy on read:** when a downstream consumer (#0011 chapter rollup, future continuity check) reads a `SceneRecord` and `agent_layer::is_stale` returns true, the consumer requests a re-transcode via the orchestrator.

The default-off auto-transcode is the conservative position. The user-experience question ("does the writer want a 30s background skill running every save?") needs real-use evidence before it becomes default-on.

### Pinning
- A `SceneRecord` can be pinned (per #0008's `pinned: bool` flag in `AgentArtifactMeta`).
- Pinned records are not invalidated by source-hash changes. The Inspector UI for pinning is out of scope here; storage support already lands in #0008.
- A pinned record produced by skill v1 is preserved verbatim even after the skill version bumps. The version bump only affects re-derivation of unpinned stale records.

### Output validation
The orchestrator (#0009) will already validate top-level shape via `SkillOutputSchema::SceneRecords`. The transcoder adds skill-specific checks before persistence:
- Every `paragraph_id` referenced must exist in the chapter's current paragraph index. References to nonexistent ids → reject the whole record with `OrchestrationError::OutputSchema { field: "paragraph_id", reason: "<id> not in chapter" }`.
- Scene `paragraph_range` must be contiguous and non-overlapping across the chapter's scenes; gaps or overlaps → reject.
- The union of all scenes' paragraphs must cover the chapter's paragraph index exactly (no omissions, no extras).
- Every `EntityId` in `pov`, `present`, `location`, `Event.actor`, `Event.object`, `EntityDelta.entity_id` must resolve to an existing entity. Unresolved → reject with the unresolved id named.

These are enforced as `SceneRecordValidationError` returned from a `validate_scene_records(records, paragraphs, entities)` helper, called by the orchestrator's typed-output persister for this skill.

### Logging
Per the orchestrator's invocation log, plus skill-specific fields written into the log's `outcome` payload:
- Scene count produced.
- Total `confidence` distribution (low / med / high counts).
- Per-scene `paragraph_range` widths (for tuning the 30-paragraph cap).

## Out of scope
- The entity-extraction skill that proposes new characters/locations from the prose. Transcoder must operate with the entity set as it currently stands; unrecognised names get `None`.
- An `agent.propose` call from inside the transcoder. Trust boundary preserved — proposing canon changes is a different skill's job.
- Chapter rollups derived from scene records — that's #0011.
- A full-fidelity hand-written gold-standard for transcoder evaluation. (See "Validation" in design notes — light human-spot-check is enough for v1.)
- Editor UI for hand-editing a `SceneRecord` (mentioned in `docs/memory-architecture.md` §8 "Transcoding fidelity" as a mitigation; deferred until we've seen real transcoder output).
- Cross-chapter scene boundaries (every chapter is transcoded in isolation in v1).

## Acceptance criteria
- [ ] `SceneRecord`, `Event`, `EntityDelta`, `ThreadRef` structs exist in `src/book/agent_layer.rs` with `serde` round-trip tests.
- [ ] `SCENE_TRANSCODE` skill is registered in the `SkillRegistry` from #0009; declared tools all resolve.
- [ ] Running `Orchestrator::run(&SCENE_TRANSCODE, ChapterRef("Awakening"))` against a fixture chapter and a stub LLM produces N records that pass `validate_scene_records` and persist to `Info/agent/scene_records/Awakening/`.
- [ ] Validation rejects: a record referencing a nonexistent `paragraph_id`; a scene gap; a scene overlap; a partial chapter cover; an unresolved EntityId — each with the named field/reason in the error.
- [ ] Re-running the transcode atomically replaces the chapter's records (no partial directory state survives a simulated mid-write panic in the test).
- [ ] Pinned records survive a re-transcode that would otherwise overwrite them.
- [ ] `agent_layer::is_stale` correctly reports stale for a record whose underlying paragraph hash changed; not stale for a pinned record.
- [ ] "Transcode scenes" button on the Chapter tab triggers the skill and shows progress.
- [ ] `auto_transcode_on_save` setting exists and defaults to `false`. When `true`, save triggers a transcode iff any paragraph hash changed.
- [ ] Per-invocation log includes scene count and confidence distribution.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why one model call per chapter, not per scene?** Within-chapter continuity (the dialogue tone of scene 3 informs scene 4's setup) is best preserved by giving the model the whole chapter at once and asking for an array. Per-scene calls would force the model to re-establish context every time at multiplied cost.
- **Why scene-as-paragraph-range, not author-tagged scenes?** Most chapters in *The Redemption Chronicles* don't have explicit scene markers. Asking the model to find scene boundaries is honest about the writer's actual format. The optional `% scene-break` comment lets the writer override when they want.
- **Why default `auto_transcode_on_save` to off?** A 30-second background skill on every save is intrusive until proven reliable. Default-on is a follow-up after we have real-use evidence the transcoder is fast and good enough not to interrupt the writing flow.
- **Why lazy re-derivation rather than eager?** Per `docs/memory-architecture.md` §5.4, eager re-derivation thrashes during active writing (every keystroke that lands a paragraph save would queue work). Lazy on read amortises the cost to when a downstream skill actually needs the record.
- **Relationship to #0004.** #0004 caches per-paragraph coach output keyed on paragraph hash. The transcoder uses the same general pattern — `source_hash` on `SceneRecord` is the equivalent — but doesn't share #0004's storage; transcoder records live in the Agent Layer, coach cache lives in `chapter.json`. They could converge later; not worth coupling now.
- **Validation strictness.** It is tempting to "fix up" near-miss model output (e.g. coerce an unresolved name into a fresh EntityId). Don't. The trust boundary requires the transcoder to fail loudly so the entity-extraction skill (separate ticket) can do the proposing — silent autocorrect from a transcoder is exactly how Author Layer canon would get polluted.
- **Validation evaluation.** A small hand-checked fixture in `tests/data/transcode/awakening/` provides 1 chapter of LaTeX + a hand-written expected `SceneRecord` array. The test asserts validation passes on the expected output and that running the orchestrator with a stub LLM that returns the expected output persists it correctly. Quality of the *model's* output is human-judged, not test-asserted (no oracle).
- **Pinned records and version bumps.** Pinning is the author's escape hatch from "I don't trust skill v2 to redo this scene right." A pinned record from skill v1 stays as-is even after v2 is registered. If the skill version changes the schema (rare), loaders accept the old schema for read-only purposes; new writes always use the current schema.
- **Token budget assumption.** A typical chapter ≈ 5k–8k tokens of prose; output of ~3–8 scenes × ~120 tokens each ≈ 1k tokens. Headroom is ample inside gemma4 even with the entity bible (#0011) loaded as system context.
