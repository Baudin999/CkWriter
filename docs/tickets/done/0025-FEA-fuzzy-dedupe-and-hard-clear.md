# 0025 — FEA: Fuzzy dedupe, hard-clear button, and AI context for dismissed flags

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0024

## Problem
The per-paragraph play button (#0024) made a pre-existing dedupe weakness
visible: when the model re-runs against the same paragraph it often picks
a different quote substring for the same observation, which produces a
different `id_hash` and slips past the
`chapter_store.records.contains_key(&id)` check. Result: dismissed flags
appear to "come back" — they don't (the dismissed records still live in
the store), but new Proposed records pile up next to them covering the
same observations. The store on `Ancient/Wua.json` for `p_2bd65496` shows
this directly: 5 Dismissed flags + 5 fresh Proposed flags whose quotes
are substrings or partial overlaps of the dismissed ones.

The writer also has no way to wipe a single paragraph's history and
re-run from scratch, and no way to tell the AI *why* a flag was
dismissed (the next run repeats the same mistake).

## Scope
- `Cargo.toml` — add `strsim` (small, no deps; needed for Jaro-Winkler /
  Sørensen-Dice token similarity).
- `src/book/suggestions.rs` —
  - `pub fn fuzzy_match_record_id(...)` pure helper: given a target
    `(pipeline, paragraph_id, normalized_quote)` and the chapter store,
    return the existing `record id` if any record in the same
    `(pipeline, paragraph_id)` matches by **bidirectional substring
    containment** OR **token-set Jaccard ≥ 0.7** on the normalized
    quote. Identity hash stays the keying scheme — we just look up by
    similarity, then adopt the matched id so re-ingest preserves status.
  - Unit tests covering: substring containment in both directions, token
    Jaccard threshold, no-match below threshold, scoping to same
    `(pipeline, paragraph_id)` (records in other paragraphs / pipelines
    are ignored).
- `src/app/coach.rs` —
  - `ingest_response`: replace the exact `contains_key` lookup with
    `fuzzy_match_record_id`. On match, do NOT insert a new record — the
    existing record's status (Dismissed / Accepted / Proposed) wins.
    Existing-id-equals-input-id also still works (degenerate case of
    substring containment).
  - `pub fn hard_clear_paragraph(&mut self, paragraph_id: &str)`: removes
    every record (any status, including Stale) whose `paragraph_id ==
    Some(paragraph_id)` from the active chapter's store, persists, and
    calls `rebuild_revisions_from_store`.
  - `start_single_paragraph_run` and `start_next_paragraph_stream`:
    extend the user prompt with a "Already reviewed — do not flag again"
    section containing every Dismissed + Accepted record for that
    `(pipeline, paragraph_id)`. (Stale records skipped — they're
    auto-swept tombstones, not deliberate writer decisions.)
- `src/ui/editor.rs` —
  - Render a trash glyph in the gutter, immediately right of the play
    glyph (between play and the dirty bar), hover-only with the same
    Y-band rule as #0024.
  - Click → `app.hard_clear_paragraph(id)`.
  - Bump `MIN_SIDE_PADDING` once more to fit the second icon.

## Out of scope
- Per-card author notes (Ticket B, file separately).
- Per-paragraph author notes (Ticket B).
- Retroactive merging of duplicates already in `Wua.json` and friends —
  the new fuzzy dedupe is going-forward-only; the writer cleans existing
  duplicates per-paragraph with the new clear button.
- Sending dismissed/accepted records to the chapter-level voice run.
- Confirmation modal on the trash button. Keep it cheap; the suggestion
  store is git-tracked, so a misfire is recoverable with `git checkout`.
- Tuning the Jaccard threshold — start at 0.7 and revisit if the
  writer reports either false matches or misses.

## Acceptance criteria
- [x] `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- [x] `cargo test` passes, with new unit tests for `fuzzy_match_record_id`
  covering: exact match, substring superset, substring subset, token
  Jaccard above threshold, token Jaccard below threshold, scope by
  pipeline, scope by paragraph_id.
- [x] Re-running the play button on a paragraph whose flags were
  previously dismissed does NOT add Proposed records that overlap the
  dismissed ones (verified manually against `Ancient/Wua.json`-style
  case).
- [x] Hovering a paragraph reveals two glyphs in the gutter: play
  (left) then trash (right), then the dirty bar.
- [x] Clicking the trash glyph removes every record for that
  `paragraph_id` from the in-memory revisions list AND from the on-disk
  suggestion JSON.
- [x] After a hard-clear + play-button run, the prompt sent to the model
  no longer carries any "do not flag" hints for that paragraph (since
  the records are gone).
- [x] Without a hard-clear, the prompt for a per-paragraph run includes
  every non-Stale record's quote under an "Already reviewed" section.

## Design notes
- **Why fuzzy over exact**: the failure mode in `Ancient/Wua.json` is
  the model picking a strict substring of the dismissed quote. Exact
  normalized-string equality can never catch that. Token-set Jaccard
  with substring containment as a fast pre-filter handles it without
  needing a heavy fuzzy-search library.
- **Why adopt the matched id, not insert a new one**: keeps the
  identity-hash invariant intact — every record's `id` is still
  `id_hash(pipeline, paragraph_id, normalized_quote_at_creation)`. The
  fuzzy match only changes *lookup*, not keying. Status history rides
  with the original record.
- **Why hard-clear includes Stale**: the writer asked. The Stale
  tombstone is useful as long as the writer might want to see what
  *was* there; once they've hit the trash button, the request is "I
  want a fresh slate", and Stale is just clutter.
- **Why send dismissed/accepted to the model**: closes the loop. Today
  the model has no memory of prior interactions, so it re-flags
  deliberate stylistic choices on every run. Even a list of quotes is
  enough signal — "don't re-raise this exact wording" is a rule the
  model can follow without needing the *why* (Ticket B will add the why
  on top).
- **Trash icon ahead of clear semantics**: a hard-delete button is
  unusual without a confirm. We're skipping the confirm because (a) the
  store is git-tracked (recovery is `git checkout`), (b) the two-glyph
  gutter is hover-only so an accidental hit needs the writer to
  hover-paragraph + click-trash deliberately, and (c) confirms add a
  modal-management rabbit hole the writer doesn't want for an iterative
  edit loop.
