# 0012 — FEA: Local vector retrieval (semantic fallback)

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0008, #0009
**Supersedes the embedding primitive of #0006** — see "Relationship to #0006" below.

## Problem
Most retrieval needs in CkWriter are answered by **structured** queries: `characters.mentions(id)`, `timeline.around(event)`, walking `SceneRecord.entity_deltas`. `docs/memory-architecture.md` §5.2 estimates this covers ~80% of cases.

The long tail is fuzzy: "scenes that feel like grief," "passages that resemble the chapter-3 confrontation in pacing," "places where Aren acts out of character." These don't pin down to typed filters; they want semantic similarity over prose.

This ticket adds local vector retrieval as a **fallback** tool exposed through the agent framework, with three explicit constraints:

1. **Built only after structured retrieval is in use** and we have evidence of the long tail. This ticket exists in backlog so the design isn't lost; do not start it before #0008–#0011 have shipped and been used.
2. **Local-only.** Embeddings via `fastembed-rs` (in-process, ONNX); no Ollama dependency for embeddings, no cloud calls.
3. **Operates on Manuscript paragraphs and `SceneRecord` summaries.** Not author notes, not Agent Layer observations — those have structured retrieval paths.

## Scope

### Embedding model
- `fastembed-rs` with `BAAI/bge-small-en-v1.5` (or equivalent ~384-dim small English model). ONNX runtime is in-process; no external service.
- Model files cached in `~/.cache/ckwriter/fastembed/` (per-user, not per-book).
- A first-time embed downloads the model (~80MB); document this in the settings panel.
- Hard rule: no embedding model is loaded until the first call to a `retrieve.semantic` tool. CkWriter's startup time is unaffected for users who never use semantic retrieval.

### Storage
- Per-book index at `Info/agent/embeddings/`:
  - `paragraphs.f32` — packed `[f32; dim]` array, one row per indexed paragraph
  - `paragraphs.json` — sidecar with `{schema, dim, model_id, rows: [{paragraph_id, chapter, source_hash}]}`
  - `scene_records.f32` + `scene_records.json` — same shape, indexed over `SceneRecord` `précis` (computed by the indexer; for #0010-produced records the indexer derives a short text-form from the structured fields).
- Schema versioning: bump `schema` in the sidecar on incompatible changes; loaders accept `schema <= CURRENT`. On `model_id` mismatch, the index is treated as invalid and rebuilt.

### Indexing
Module `src/agent/embeddings.rs`:
```rust
pub struct VectorIndex { /* mmap of .f32 + sidecar */ }

impl VectorIndex {
    pub fn open_or_create(book_root: &Path, kind: IndexKind) -> Result<Self>;
    pub fn upsert(&mut self, items: &[(IndexedItemRef, &str)]) -> Result<()>;
    pub fn remove(&mut self, refs: &[IndexedItemRef]) -> Result<()>;
    pub fn search(&self, query_text: &str, k: usize, scope: SearchScope) -> Result<Vec<Hit>>;
}

pub enum IndexKind { Paragraph, SceneRecord }
pub enum SearchScope { WholeBook, Chapter(String), Chapters(Vec<String>) }
pub struct Hit { pub item: IndexedItemRef, pub score: f32, pub snippet: String }
```
- `search` is brute-force cosine in v1 — for a 120k-word novel (~3000 paragraphs at ~384 dims = 4.6MB of f32) brute force is ~1ms. ANN structures are unnecessary until the corpus grows past ~50k items.
- Indexing strategy:
  - **On manual trigger.** Two new buttons under settings: "Build paragraph index" and "Build scene-record index." Each runs across the whole book, embedding every item, writing the index, replacing any prior file atomically.
  - **Incremental update.** When a chapter saves, dirty paragraphs (changed hash) are re-embedded and upserted; deleted paragraphs are removed. Per-paragraph cost: one embed + one mmap write — fast enough to run synchronously on save.
  - **No on-startup auto-build.** First-time setup is explicit (the user opts in by clicking the button).
- Idempotent: upserting an unchanged `(paragraph_id, source_hash)` pair is a no-op.

### Tool registration
Add to the tool registry from #0009:
```
retrieve.semantic        — { query, k, scope } → { hits: [{paragraph_id, score, snippet}] }
retrieve.entity_mentions — { entity_id, scope } → { hits: [{paragraph_id}] }   (structured, no embeddings; thin wrapper over characters.mentions etc.)
```
- `retrieve.semantic` permission: `Retrieve`.
- Tool description (≤120 chars per #0009 lint): `"Find paragraphs or scenes whose meaning is similar to the query. Use only when structured filters can't pin it down."`
- The tool is **off by default per skill**; a skill must explicitly list `retrieve.semantic` in its declared `tools`. This makes it deliberate which skills get to do fuzzy lookups (they pay the cost in latency and risk of irrelevant hits).

### Settings & UX
- Per-book setting `embeddings_enabled: bool` (default `false`). If false, the tool returns an empty hit list with a one-time log warning and never loads the model. Skills that declared `retrieve.semantic` still run, just without semantic results.
- Settings panel section "Local embeddings": status (built / not built / stale), buttons for build/rebuild, model name + size, last index time, item counts.
- A "stale" indicator: index is stale when any indexed paragraph's current hash differs from the indexed `source_hash`. Surfacing in the panel; rebuild is manual.

### Ollama is not required for this feature
Explicit non-dependency on Ollama for embeddings — `fastembed-rs` is in-process. The Ollama integration is only used for skill model calls (#0009).

### Relationship to #0006
`#0006-FEA-embedding-fuzzy-dedup` (in `todo/`) proposes embedding-based dedup of suggestion flags within a paragraph. It currently specifies `nomic-embed-text` via Ollama as the embedding source.

This ticket changes the embedding primitive: `fastembed-rs` in-process with `bge-small-en-v1.5`. **#0006 should be reframed as a consumer of this ticket's `VectorIndex`** (or a sibling per-suggestion index using the same embedder) rather than wiring Ollama. Two paths:

- **Path A (preferred):** When #0012 lands, update #0006's design notes to use `fastembed-rs` via the embedder primitive from this ticket; the dedup index is a separate small per-paragraph index, not the manuscript paragraph index. #0006 stays a separate ticket but reuses this embedder.
- **Path B:** Close #0006 if paragraph-focused coaching (#0007) makes the residual duplicate rate inside a paragraph negligible. Decide based on usage data after #0007 ships.

Either path leaves this ticket's scope unchanged. The decision belongs to whoever picks up #0006 next; this ticket flags the dependency so they don't ship two embedders.

## Out of scope
- ANN index structures (HNSW, IVF). Brute-force is fine until the corpus grows.
- Indexing Author Layer entity descriptions (entity search uses the typed `characters.search` substring path; semantic search over entities is a separate ticket if ever needed).
- Indexing Agent Layer observations or summaries.
- Cross-book search.
- Re-ranking with an LLM.
- Automatic index build on first open.
- Embedding fuzzy-dedup of suggestion flags — that's #0006's territory; see "Relationship to #0006" above.

## Acceptance criteria
- [ ] `VectorIndex::open_or_create` produces a fresh index on a book with no embeddings; `search` returns an empty hit list.
- [ ] "Build paragraph index" button embeds every paragraph in the open book; on completion, `VectorIndex::search("a query", 5, WholeBook)` returns 5 hits sorted by score.
- [ ] Incremental save: editing one paragraph re-embeds exactly one item; index sidecar reflects the new `source_hash`.
- [ ] Deleting a paragraph removes its row from the index (verified by absence in subsequent search).
- [ ] `retrieve.semantic` registered as a tool with permission `Retrieve` and description ≤120 chars.
- [ ] A skill that does **not** declare `retrieve.semantic` in its `tools` cannot call it (orchestrator rejects with `OrchestrationError::ToolError { name, source: "tool not in skill's allowed set" }`).
- [ ] `embeddings_enabled = false` makes `retrieve.semantic` return empty hits without loading the model. Verified by checking the model is not in memory after a call (proxy: model file not opened).
- [ ] Stale paragraphs are detectable via the sidecar; the settings panel shows a stale count.
- [ ] Search latency on a 3000-paragraph fixture is < 50ms (brute force should be ~1ms; this is a sanity ceiling).
- [ ] Index file format round-trips cleanly: open → search → re-open new instance → same results.
- [ ] Model download on first use is deferred (no model loading in `Book::open` or `VectorIndex::open_or_create`).
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why brute-force cosine?** A novel-scale corpus (≤10k paragraphs) is too small for ANN to pay off. Brute force is correct, simple, and fast enough. Revisit if a series of novels or a multi-book project pushes us past ~50k items.
- **Why `bge-small-en-v1.5`?** ~80MB download, ~384 dims, well-benchmarked, MIT-licensed via `fastembed-rs`. Larger models (mpnet-base, etc.) cost more disk + latency for marginal recall gains on short prose. Re-evaluate if recall on real queries disappoints.
- **Why mmap the f32 file?** Avoids parsing JSON for vectors; cosine math runs directly over the mmap. Sidecar JSON keeps metadata human-inspectable.
- **Why not auto-build on first open?** Embedding 3000 paragraphs takes ~30s on a typical CPU. Doing this silently on first open of a book would be a surprise. Explicit opt-in via the button is honest about the cost.
- **Why deferred model loading?** Users who never use semantic retrieval should pay zero startup cost. Lazy load on first `retrieve.semantic` call (or on the build button click) is the right default.
- **Why per-book index, not per-user?** Different books have different prose; a global index would mix retrieval surfaces across novels. Per-book under `Info/agent/embeddings/` keeps the substrate aligned with the rest of the Agent Layer.
- **Why scope as an enum, not arbitrary filters?** Common scopes (whole book, one chapter, list of chapters) cover ~all use cases and are cheap to apply during the brute-force scan. Adding entity-mention scope is a follow-up if needed.
- **`retrieve.entity_mentions` is structured, not vector.** It lives next to `retrieve.semantic` in the tool registry because callers reach for it from the same mental category ("find me passages about X"), but its implementation is a thin wrapper over `characters.mentions(id)`. No embeddings involved.
- **The `Retrieve` permission tag exists in #0009 but had no tools using it.** This ticket is the first user.
