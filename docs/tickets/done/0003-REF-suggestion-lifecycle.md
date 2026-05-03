# 0003 — REF: Unified suggestion lifecycle

**Type:** REFACTOR
**Created:** 2026-05-03
**Depends on:** #0002

## Problem
Dismissals today live in `Info/coach-dismissals.json` (single per-book file, keyed `chapter_name → pipeline_label → set<normalized_quote>`), applied as a post-filter at ingest time. Accepted suggestions are dropped from the in-memory `revisions` list with no record. There is no notion of "stale" (paragraph changed and quote no longer present). Consequences:

- No durable record of accepted suggestions — can't reconstruct what the coach has helped with
- No automatic cleanup when paragraphs are rewritten — dismissals for vanished quotes accumulate forever
- The "filter dismissed" toggle is implemented as a coarse pre-ingest skip, which means dismissals can't be selectively re-surfaced for review during sealing
- No identity for a flag, so #0004 (per-paragraph caching) and #0006 (embedding dedup) have nothing to key against

This refactor introduces a single per-chapter lifecycle store keyed by deterministic suggestion identity, with explicit `proposed | accepted | dismissed | stale` states.

## Scope

### New persistence: `Info/suggestions/<folder>/<name>.json`
One file per chapter, mirroring the path layout of `Info/chapters/<folder>/<name>.json`. Shape:

```rust
struct SuggestionRecord {
    id: String,                  // hex(blake3(pipeline + paragraph_id + normalized_quote))
    pipeline: String,            // pipeline label ("voice" | "show" | "prose" | "spelling")
    kind: String,                // FlagKind label ("spelling" | "punctuation" | "grammar" | "")
    paragraph_id: Option<String>,// None when anchoring failed at ingest time
    quote: String,               // raw quote as the model emitted it (for re-anchoring on load)
    normalized_quote: String,    // input to the identity hash and to the stale check
    why: String,
    suggestion: String,
    status: Status,              // Proposed | Accepted | Dismissed | Stale
    created_at: i64,             // unix seconds
    resolved_at: Option<i64>,    // set when status leaves Proposed
}

// On disk: BTreeMap<String, SuggestionRecord> keyed by id.
// Map (not Vec) so "does id already exist?" is O(log n) per ingested flag.
```

A new `book::suggestions` module owns load/save and the lifecycle ops. `Book` gets a `suggestions: SuggestionStore` field that replaces `dismissals: Dismissals`.

### `Revision` struct (in `src/llm/revision.rs`)
Adds two fields:
- `suggestion_id: String` — the persisted identity
- `paragraph_id: Option<String>` — runtime convenience; #0004 and #0005 both need cheap access from a panel card

### Ingest (`app/coach.rs::ingest_response`)
For each parsed flag:
1. Anchor in `editor_text` (existing logic).
2. Resolve `paragraph_id` from `current_paragraphs[].char_range` containing the anchor's start byte. If anchoring failed, `paragraph_id = None`.
3. Compute `id = blake3_hex(pipeline + paragraph_id_or_empty + normalize(quote))`.
4. If `id` already in the store: leave the existing record untouched (preserves status history; ignores the duplicate).
5. Else: insert a new record with `status = Proposed`, `created_at = now`.
6. Build the in-memory `Revision` (with `suggestion_id`, `paragraph_id`) so the panel renders it.

After all flags are ingested, run the auto-stale sweep (see below) and save the store.

### Auto-stale sweep
On every chapter open and every successful ingest, walk `Proposed` records for the current chapter and mark `Stale` (with `resolved_at = now`) if either:
- `paragraph_id` is `Some` and that id no longer appears in `current_paragraphs`, OR
- `paragraph_id` is `Some` and `current_paragraphs[paragraph_id].text` no longer contains `normalized_quote` after applying `dismissals::normalize`

Records with `paragraph_id = None` are exempt — without an anchor we can't make this judgement.

### Accept / Dismiss
- `accept_revision(id)`: existing in-place text replacement, plus update store: `status = Accepted`, `resolved_at = now`.
- `dismiss_revision(id)`: drop from in-memory `revisions`, plus update store: `status = Dismissed`, `resolved_at = now`. Recording is unconditional — independent of any UI toggle.

### Chapter-switch rehydration
When opening a chapter, rebuild the in-memory `revisions` list from the store:
- Always include records with `status = Proposed`
- If `coach_filter_dismissed == false`, also include `Dismissed` (rendered with a visual differentiator — that UI is part of this ticket; suggestion: dim the card and prepend a "dismissed" pill so the writer can tell them apart and click to un-dismiss)

For each rehydrated record, run `revision::anchor()` on `editor_text` using the raw `quote`. Records that fail to anchor still appear, with `anchor = None`, sorted to the bottom (matches today's behaviour for ingest-time anchor failure).

### `coach_filter_dismissed` toggle
Stays. Renamed semantically from "post-filter at ingest" to "panel visibility filter for `Dismissed` records." Default unchanged (true). The settings field, the checkbox in `scope_panel.rs`, and the UI label remain. What changes:
- Recording is unconditional (already noted above)
- Rehydration consults the toggle to decide whether to load `Dismissed` for display
- Toggling at runtime triggers a rehydration of the current chapter (cheap: just walks the store)

This supports the writer's two modes: drafting (toggle on, signal-only) and sealing (toggle off, reconsider every dismissal).

### Migration
On `Book::open`, if `Info/coach-dismissals.json` exists:
1. For each `chapter_name → pipeline → quote` triple:
   - Load that chapter's `.tex` from disk and parse paragraphs (using `paragraphs::parse_and_match` against an empty prior, since we don't have stored paragraph metadata pre-#0002 either way; #0002's first-open seeding will assign ids deterministically from the same content).
   - Search each paragraph's text for `normalized_quote`. First match (in source order) → that paragraph's `id`.
   - No match → `paragraph_id = None`. **Preserve the entry anyway** with `status = Dismissed` — the writer's intent to dismiss is durable. Log a warning, do not drop.
2. Write per-chapter suggestion files with all migrated entries.
3. **Rename** the legacy file to `Info/coach-dismissals.json.migrated` (do not delete). On a subsequent open, the presence of `.migrated` is a no-op signal that migration already ran.

### Removed
- `book::dismissals::Dismissals::record` and its caller path in `dismiss_revision`
- The pre-ingest filter loop in `ingest_response` (lines around 215–225)

The `book::dismissals::normalize` function is **kept** — the suggestions module imports it. Identical normalization rules between old and new code.

## Out of scope
- UI for browsing accepted/stale history
- Embedding-based fuzzy dedup (#0006)
- Cross-chapter suggestion search
- A `Superseded` status — overlap-detection heuristics are not worth designing v1; revisit if it actually bites

## Acceptance criteria
- [x] One unified suggestions file per chapter at `Info/suggestions/<folder>/<name>.json`
- [x] Legacy `coach-dismissals.json` is migrated and renamed to `.migrated`; no migration data is lost (entries that can't resolve a paragraph are preserved with `paragraph_id = None`)
- [x] Re-running a pipeline that produces a previously seen flag yields one record, not two (identity dedupe; `created_at` of the first is preserved)
- [x] Auto-stale fires when a paragraph_id is removed; auto-stale fires when a paragraph_id stays but the normalized_quote disappears from its text
- [x] `accept_revision` writes `status = Accepted` and a `resolved_at`; `dismiss_revision` writes `status = Dismissed` and a `resolved_at`
- [x] Recording a dismissal is unconditional — happens whether `coach_filter_dismissed` is true or false
- [x] Toggling `coach_filter_dismissed` at runtime adds/removes dismissed cards from the panel without re-running any pipeline
- [x] Dismissed cards in the panel are visually distinguishable from proposed cards (dimmed + "dismissed" pill) and a click un-dismisses (status returns to Proposed, `resolved_at` cleared)
- [x] On chapter open, the panel is rehydrated from the store before any pipeline runs
- [x] Unit tests: identity stability, stale-by-missing-paragraph, stale-by-quote-disappeared, migration parser, rehydration anchoring, un-dismiss flow
- [x] `cargo clippy --all-targets` and `cargo test` return zero errors and zero warnings

## Design notes
- **Why per-chapter files, not a per-book file?** The store grows over time (resolved entries are kept). Bundling all chapters into one file means every chapter's lifecycle write rewrites the whole book's history. Per-chapter files keep writes scoped.
- **Why not embed in `chapter.json`?** That file is on the hot path — every chapter save rewrites it. Suggestion lifecycle records can grow into the hundreds; keep the hot file small.
- **Identity hash includes `paragraph_id`**: a flag that "moves" because its paragraph got a fresh id (no Jaccard match) gets a new identity. That's the correct semantic — the writer effectively rewrote it, and the model's flag on the new prose is a fresh judgment, not a duplicate.
- **`paragraph_id = None` flags can't be lifecycle-managed beyond accept/dismiss.** They never auto-stale. Acceptable: today's anchor failure rate is already small, and "doesn't auto-clean" is strictly better than "drops on the floor."
- **Why blake3?** Already a dependency from #0002 (`paragraphs::hash_normalized`). No new crates.
- **Migration runs at most once.** The `.migrated` rename is the idempotency marker. No migration version field needed v1.
- **Un-dismiss flow:** clicking a dismissed card in panel-when-toggle-off flips its status back to `Proposed` and clears `resolved_at`. Matches the sealing-pass mental model: "I was wrong to dismiss this — bring it back."
- **Future hooks:** `paragraph_id` on every record is what #0004 keys per-paragraph cache invalidation against, and what #0005 reads to short-circuit "locked → no flags."
