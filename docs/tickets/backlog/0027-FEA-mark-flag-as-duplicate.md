# 0027 — FEA: Manual "same as" flag-duplicate resolution

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0025

## Problem
The model occasionally re-flags a complaint the writer has already dismissed, in vocabulary that survives both #0025's Jaccard 0.7 dedup AND the prompt-side "do not flag again" instruction in `build_user_with_history`. When this happens, the writer has no way to teach the dedup. Their only options today: dismiss the new flag (which adds it to the dismissal list as a separate record covering the same observation) or live with the noise.

This adds writer-curated, supervised dedup. One button per coach card: "Same as..." — pick the prior record this is a duplicate of. Future runs match the new pattern via the chosen record's alias list and drop it automatically. No model, no embeddings — just a growing list of writer-confirmed equivalences. This is the cheap counterpart to #0006's automatic embedding approach (which was closed unbuilt because the auto-dedup we have already handles every case the writer has seen).

## Scope
- `src/book/suggestions.rs` — add `aliases: Vec<String>` field to `SuggestionRecord`. `#[serde(default)]` so existing chapter stores deserialize cleanly. `fuzzy_match_record_id` extended: when comparing a new flag to an existing record, also check each entry in `record.aliases` for substring containment (bidirectional) or Jaccard ≥ `FUZZY_JACCARD_THRESHOLD`. Substring on alias scores 1.0 just like a primary-quote substring; Jaccard on alias scores its raw fraction. Tie-breaking unchanged.

- `src/ui/scope_panel.rs` — on a Proposed coach card, add a "Same as..." button next to Apply / Dismiss. Click → inline picker (collapsible section under the card body) listing other non-Stale records for the same `(pipeline, paragraph_id)`, sorted most-recent first, showing a one-line quote preview + truncated `why` (≤80 chars). Eligible: any non-Stale record whose id ≠ this card's id. Click an entry → handler call. Empty list → button is disabled with hover text "no other flags on this paragraph for {pipeline}".

- `src/app/coach.rs` — `pub fn merge_revision_into_record(&mut self, revision_id: u32, target_record_id: &str)`: marks the revision's underlying record as Dismissed AND appends the revision's `normalized_quote` to the target record's `aliases`. Persists via the existing chapter-store save path. Idempotent: if the alias is already present (string equality after normalization), don't append.

- Unit tests in `src/book/suggestions.rs`:
  - `fuzzy_match_with_alias_substring_match` — primary quote doesn't match but an alias does, by substring.
  - `fuzzy_match_with_alias_jaccard_match` — primary quote doesn't match but an alias does, by Jaccard ≥ threshold.
  - `fuzzy_match_alias_respects_pipeline_scope` — an alias on a Spelling record does not match an incoming Prose flag.
  - `merge_appends_alias_idempotent` — calling merge twice with the same normalized quote doesn't duplicate the alias entry.
  - `aliases_round_trip_through_chapter_store` — serialize → deserialize preserves aliases; missing field on legacy data deserializes as empty.

## Out of scope
- Cross-paragraph picker. The picker shows only same `(pipeline, paragraph_id)` for v1; cross-paragraph reuse can be a follow-up if it turns out the writer wants to merge across paragraph boundaries.
- Cross-pipeline picker. A spelling flag can never be merged into a prose record (and vice versa) — pipeline scoping is a feature, not a limitation: a complaint about spelling is genuinely different from a complaint about telling-not-showing, even if the quoted span is identical.
- Bulk merge ("mark all of these as duplicates of X").
- Undo / un-merge UI (remove an entry from `aliases`). For v1: edit the chapter store JSON by hand if a merge was wrong.
- Surfacing the alias list visually on the original card. The alias is invisible after merging — it just affects the next coach run's dedup. If the writer ever wants to audit "what aliases does this record carry," that's a follow-up.
- Sending alias text to the AI history context in `build_user_with_history`. The original record's `normalized_quote` already goes; aliases would bloat the prompt for marginal gain. Reconsider if needed.

## Acceptance criteria
- [ ] "Same as..." button visible on every Proposed coach card.
- [ ] Click → picker shows other non-Stale records on the same `(pipeline, paragraph_id)`, most-recent first, with quote + truncated `why` preview. Empty list disables the button.
- [ ] Pick one → the new record's status becomes Dismissed; the chosen record's `aliases` grows by one entry; chapter store is persisted.
- [ ] Re-run the same coach pipeline on the same paragraph: a flag whose normalized quote substring-matches OR Jaccard-matches (≥ threshold) the new alias is dropped via `fuzzy_match_record_id`; no new Proposed card surfaces for that complaint.
- [ ] `aliases: Vec<String>` round-trips through chapter store JSON. Old stores (no field) deserialize as empty.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` clean.

## Design notes
- Aliases are matched by the same rules as `normalized_quote`: bidirectional substring containment OR Jaccard ≥ threshold. The alias list piggybacks on the existing matcher rather than getting its own scoring.
- Alias matching is **per-record**, not global. A record's alias list only catches duplicates of *that record's* observation. If the writer says "flag X is the same as flag Y," they're not saying "flag X is the same as everything that ever existed" — and a record's alias should only widen its own match radius.
- The `aliases` vec is unbounded in v1. For a typical chapter (<200 records, alias list <10 per record), the matcher's O(records × aliases) per ingest is negligible. If it ever bites, precompute a token-set fingerprint per alias and cache it on the record; not worth doing now.
- Picker UX: lean on existing card visual language. The button is a third action next to Apply / Dismiss; the picker is a collapsible inline section, not a popup, so it survives panel scroll without floating positioning. Same-paragraph scoping keeps the list short enough that no search/filter UI is needed.
- Why this exists in addition to #0025's auto-dedup: auto-dedup catches the model picking a different *substring* of the same complaint. #0027 catches the model picking a different *vocabulary* — paraphrase. The writer's eye is the cheapest paraphrase detector available; this ticket just records what it sees.
