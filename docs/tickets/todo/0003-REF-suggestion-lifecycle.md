# 0003 — REF: Unified suggestion lifecycle

**Type:** REFACTOR
**Created:** 2026-05-03
**Depends on:** #0002

## Problem
Dismissals today live in `coach-dismissals.json`, keyed by chapter+pipeline, applied as a post-filter. Accepted suggestions are dropped immediately with no record. There is no notion of "stale" (paragraph changed and quote no longer present) or "superseded" (newer flag overlaps an older one). Consequences: no audit trail of what the coach has flagged, no way to show "what did I dismiss?", no automatic cleanup when paragraphs change, dismissal storage grows unbounded.

This refactor unifies dismissals/accepts/proposals under a single lifecycle keyed by suggestion identity.

## Scope
- New unified `Info/suggestions/<folder>/<name>.json` per chapter (replaces `coach-dismissals.json`)
- Each entry: `{ id, pipeline, paragraph_id, normalized_quote, why, suggestion, status, created_at, resolved_at }`
- `id = hex(blake3(pipeline + paragraph_id + normalized_quote))` — deterministic, dedupes across runs
- `status: proposed | accepted | dismissed | stale | superseded`
- `Revision` struct in `coach.rs` gains `suggestion_id`
- `dismiss_revision` writes `status = dismissed` (replaces the `Dismissals::record` path)
- `accept_revision` writes `status = accepted`
- On every coach ingest:
  - For each new flag: compute `id`; if already exists, leave the existing entry alone (don't overwrite); else insert new with `status = proposed`
  - Auto-stale sweep: any `proposed` entry whose `paragraph_id` no longer exists or whose `normalized_quote` no longer appears in the paragraph → `status = stale`
- Display filter: only show `proposed` in the panel (matches current "filter dismissed" toggle behavior, but now unconditional)
- Migration on first open: if `coach-dismissals.json` exists, convert each entry to a `dismissed` suggestion (`paragraph_id` = best-match against current paragraphs; if no match, drop the entry with a warning), then delete the old file

## Out of scope
- UI for browsing dismissed/accepted history — file a follow-up if wanted
- Embedding-based fuzzy dedup — that's #0006
- Cross-chapter suggestion search

## Acceptance criteria
- [ ] One unified suggestions file per chapter; old `coach-dismissals.json` migrated and removed
- [ ] Identity hash dedupes a flag re-emitted on the next run (single entry, not two)
- [ ] Stale auto-fires when paragraph hash changes and the quote disappears
- [ ] Accept and dismiss both persist with timestamps
- [ ] Existing "filter dismissed" toggle either repurposed ("show resolved suggestions in panel") or removed; choice documented in this ticket
- [ ] Unit tests: identity stability, stale detection, migration from `coach-dismissals.json`
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- `created_at` / `resolved_at`: unix seconds. Useful for any later activity-log feature.
- `paragraph_id` is required — depends on #0002. Suggestions without a paragraph_id can't be lifecycle-managed.
- In-memory `revisions` Vec stays as the panel view; rebuilt from `proposed` entries on chapter switch and on ingest.
- blake3 chosen over sha256: faster, same crate already? — check before locking; sha256 is acceptable fallback.
