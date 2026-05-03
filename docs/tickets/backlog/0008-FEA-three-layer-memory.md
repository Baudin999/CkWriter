# 0008 — FEA: Three-layer memory + typed entity accessors + Agent Layer storage

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0001, #0002

## Problem
Today CkWriter has the Manuscript (chapters + paragraph index) and an embryonic Author Layer (`Info/Characters/`, `Info/Locations/`, `Info/Events/`, `Info/Timeline/`), but:

- There is no Agent Layer — no place for the model to persist what it has *figured out* (scene transcodings, observations, summaries, retrieval traces) without polluting either the manuscript or canon entity files.
- The accessor surface is generic (`Entities::get(id)`, `entities_of(kind)`); skills cannot reason in terms of `characters.mentions(id)` or `timeline.around(event)`.
- There is no enforced trust boundary. Any code path that touches `Entities::save` can stomp canon. Without a `propose → confirm` valve, an autonomous skill could silently rewrite the bible.
- Without typed accessors and an Agent Layer, every later ticket (#0009 skill framework, #0010 transcoder, #0011 always-loaded context, #0012 vector retrieval) has nowhere to plug in.

This is the substrate for the rest of the bundle. See `docs/memory-architecture.md` §2–§3 for the design.

## Scope

### Module layout
- New module `src/book/agent_layer.rs` — Agent Layer storage primitives.
- New module `src/book/accessors.rs` — typed read-only accessors over the Author Layer (`characters`, `locations`, `factions`, `items`, `magic_rules`, `timeline`).
- New module `src/book/proposals.rs` — proposal queue for Agent → Author promotion.
- Existing `src/book/entity.rs` is extended (new `EntityKind` variants), not replaced — see "Out of scope".

### EntityKind extension
Add to `EntityKind`:
- `Faction` (folder `Factions/`)
- `Item` (folder `Items/`)
- `MagicRule` (folder `MagicRules/`)

`EntityKind::Timeline` already exists and is reused for `TimelineEvent`. The existing god-`Entity` struct is kept in v1; the typed accessors expose per-kind views over it. Splitting `Entity` into `Character`, `Location`, etc. is deferred (see Out of scope) — it is a large refactor with no v1 user-visible benefit.

### Typed accessor API (read-only)
In `src/book/accessors.rs`:
```rust
pub struct Accessors<'a> { book: &'a Book }

impl<'a> Accessors<'a> {
    pub fn characters(&self) -> KindView<'a> { ... }
    pub fn locations(&self) -> KindView<'a> { ... }
    pub fn factions(&self) -> KindView<'a> { ... }
    pub fn items(&self) -> KindView<'a> { ... }
    pub fn magic_rules(&self) -> KindView<'a> { ... }
    pub fn timeline(&self) -> TimelineView<'a> { ... }
}

pub struct KindView<'a> { /* kind, entities */ }
impl<'a> KindView<'a> {
    pub fn get(&self, id: &str) -> Option<&Entity>;
    pub fn list(&self) -> Vec<&Entity>;
    pub fn search(&self, query: &str) -> Vec<&Entity>;        // case-insensitive substring on name + aliases
    pub fn mentions(&self, id: &str) -> Vec<ParagraphRef>;    // backed by §"Mentions index"
}

pub struct TimelineView<'a> { /* ... */ }
impl<'a> TimelineView<'a> {
    pub fn get(&self, id: &str) -> Option<&Entity>;
    pub fn range(&self, from: &str, to: &str) -> Vec<&Entity>;     // lexical sort on `when` field; v1 sufficient
    pub fn around(&self, id: &str, n: usize) -> Vec<&Entity>;      // ±n events around the given event
}
```
- All accessors are `&self` only — no `mut` accessor exists. Mutation goes through `Book::save_entity` (existing) for direct author edits, or through the proposal queue for agent-originated changes.
- `book.accessors()` returns an `Accessors<'_>` borrow of `&self.entities`.

### Mentions index
Per-paragraph reverse map from `EntityId → Vec<ParagraphId>`. Persisted at `Info/agent/mentions.json`:
```json
{ "schema": 1, "by_entity": { "yara": ["p_a1b2c3d4", "p_e5f6..."] } }
```
Built by an entity-extraction skill in a later ticket; in #0008, ship the storage + accessor only, with an `agent.set_mentions(entity_id, refs)` write path for the future skill to use. Empty index is the v1 default.

### Agent Layer storage
- Root directory: `Info/agent/`
- Per-artifact-kind subdirectory:
  - `Info/agent/scene_records/<chapter>/<scene_id>.json` — populated by #0010
  - `Info/agent/chapter_rollups/<chapter>.json` — populated by #0011
  - `Info/agent/observations.json` — single file, append-only
  - `Info/agent/retrieval_traces.json` — single file, append-only with FIFO eviction at 1000 entries
  - `Info/agent/summaries/<topic_hash>.json` — content-addressable
  - `Info/agent/mentions.json` — see above
- Common metadata struct embedded in every artifact:
  ```rust
  pub struct AgentArtifactMeta {
      pub schema: u32,                   // bump on incompatible change
      pub source_hash: String,           // blake3-hex-16 of concatenated source paragraphs
      pub source_refs: Vec<String>,      // paragraph IDs
      pub created_at: i64,               // unix seconds
      pub created_by_skill: String,      // e.g. "scene_transcode@1"
      pub model_id: String,              // e.g. "gemma4:27b"
      pub confidence: Confidence,        // Low | Medium | High
  }
  pub enum Confidence { Low, Medium, High }
  ```
- Every artifact struct embeds `AgentArtifactMeta` as a `meta: AgentArtifactMeta` field (flat composition; `#[serde(flatten)]` on read for forward-compat with future fields at top level).

### Staleness
- New helper `agent_layer::is_stale(meta: &AgentArtifactMeta, paragraphs: &[Paragraph]) -> bool`:
  - For each `paragraph_id` in `source_refs`, look up the current paragraph hash; recompute `expected_source_hash = blake3_concat(...)` over current hashes; compare to `meta.source_hash`.
  - Missing paragraph IDs (deleted) → stale.
- Helper exposed; the orchestrator (#0009) and consumers (#0010, #0011) call it. No automatic invalidation in v1 — staleness is computed on read.
- A `pinned: bool` flag on artifacts; `is_stale` always returns `false` when `pinned == true`. Authors pin via the inspector (UI in a later ticket; storage support lands here).

### Proposal queue (Agent → Author promotion valve)
New `src/book/proposals.rs`:
- File: `Info/agent/proposals.json`
- Shape:
  ```rust
  pub struct Proposal {
      pub id: String,                    // ulid-like, monotonic
      pub kind: ProposalKind,            // EntityCreate | EntityFieldUpdate | RelationAdd | RelationRemove
      pub target: ProposalTarget,
      pub change: serde_json::Value,     // typed per kind, validated on apply
      pub reason: String,                // skill-provided justification
      pub created_at: i64,
      pub created_by_skill: String,
      pub status: ProposalStatus,        // Pending | Confirmed | Rejected
      pub resolved_at: Option<i64>,
  }
  ```
- Public API:
  - `Proposals::queue(&mut self, proposal: Proposal) -> Result<ProposalId>`
  - `Proposals::confirm(&mut self, id: &str, book: &mut Book) -> Result<()>` — applies the change to the Author Layer and marks resolved
  - `Proposals::reject(&mut self, id: &str, reason: &str) -> Result<()>`
  - `Proposals::pending(&self) -> Vec<&Proposal>`
- `Book::save_entity` is **not** restricted in v1. The trust boundary is enforced at the agent runtime (#0009), where skills are given a tool registry that does not include direct entity writes — only `author.propose`. Document this clearly in `accessors.rs` and `proposals.rs`.

### Wiring into `Book`
- `Book` gains:
  ```rust
  pub agent_layer: AgentLayer,         // root + lazy artifact loaders
  pub proposals:   Proposals,
  ```
- Both are loaded in `Book::open`; both create `Info/agent/` if missing.
- `Book::accessors()` returns an `Accessors<'_>`.

### Folder bootstrap
- On `Book::open`, ensure `Info/Factions/`, `Info/Items/`, `Info/MagicRules/`, and `Info/agent/` exist. Idempotent. Existing `Entities::load` already iterates `EntityKind` so adding the new kinds picks up files automatically.

## Out of scope
- Splitting the god-`Entity` struct into per-kind structs (`Character`, `Location`, etc.). The typed accessors give the agent the typed *interface* it needs; the storage shape is a separate refactor and can land later without breaking accessor callers.
- The skill framework / tool registry / orchestrator — that's #0009.
- The first concrete skill (`scene_transcode`) — #0010.
- Entity-extraction skill that populates `mentions.json` — separate ticket.
- Inspector UI for proposals, pinning, or stale-badge display — UI tickets follow once #0009–#0011 land.
- Vector retrieval / embeddings — #0012.
- Any change to coach pipelines — they continue to use whole-chapter prose until #0011 swaps in always-loaded context.

## Acceptance criteria
- [ ] `EntityKind` has `Faction`, `Item`, `MagicRule` variants; folder names match scope spec.
- [ ] On `Book::open`, `Info/Factions/`, `Info/Items/`, `Info/MagicRules/`, and `Info/agent/` are created if missing.
- [ ] `book.accessors().characters().get("yara")` returns the Yara character; `.list()` returns all characters sorted by name.
- [ ] `book.accessors().timeline().around("event_storm", 2)` returns the 5 events centred on `event_storm` ordered by `when`.
- [ ] `Accessors` exposes no mutation method; only `&self` borrows. Verified by a compile-time test (a `fn _no_mut(a: &Accessors)` that calls every accessor).
- [ ] Round-trip: write a `SceneRecord` skeleton (with empty `beats`, etc.) via `agent_layer::save_scene_record`; reload via `agent_layer::load_scene_record`; equal.
- [ ] Round-trip: queue a proposal, list it as pending, confirm it, observe the Author Layer mutation.
- [ ] `agent_layer::is_stale` returns `true` when any `source_refs` paragraph hash has changed; `false` for unchanged; `false` for `pinned == true` regardless of source hashes.
- [ ] Empty `Info/agent/mentions.json` round-trips; `agent.set_mentions(id, refs)` persists; accessor `characters.mentions(id)` returns the persisted refs.
- [ ] Existing entity files (`Info/Characters/*.json`) load unchanged — no schema migration required.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why extend the existing `Entity` struct instead of splitting per-kind?** v1 cost/benefit: the typed accessors give the agent the API it needs without rewriting every UI consumer. Splitting can happen later as a pure refactor, gated on a clippy/arch rule that forbids referencing kind-specific fields outside that kind's accessor view.
- **Why proposals as a separate file rather than per-entity?** The proposal queue is a workflow object, not an entity property; lifetime is bounded (resolved proposals can be archived after N days). One file keeps the inspector UI's "pending" view a single load.
- **Why `confidence` as a 3-value enum, not a float?** The model's self-rated confidence is not a calibrated probability; a coarse bucket is honest about the precision and avoids tempting downstream code to filter on `confidence > 0.73`.
- **Schema versioning.** Every persisted struct in `Info/agent/` carries `schema: u32`. Loaders accept `schema <= CURRENT_SCHEMA`; writers always emit `CURRENT_SCHEMA`. Bump on incompatible field changes; add `#[serde(default)]` for compatible additions.
- **Why no auto-invalidation on edit?** The save path already runs the paragraph matcher (#0002). Computing `is_stale` for every Agent Layer artifact on every save would scale with N artifacts; doing it on read scales with N reads of stale artifacts (typically zero — the orchestrator re-derives lazily). Re-evaluate if read latency suffers.
- **Trust boundary enforcement is in #0009**, not here. This ticket ships the proposal storage; the rule "skills cannot call `Book::save_entity`" is enforced by the tool registry the orchestrator hands to skills.
