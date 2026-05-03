# 0024 — FEA: Per-paragraph play button

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0004, #0023

## Problem
Today the writer can only run show / prose / spelling at the chapter level
(top-bar buttons). With per-paragraph caching landed (#0004) and the
four-state gutter landed (#0023), the writer can *see* which paragraph is
dirty or has unresolved feedback — but to recheck a single paragraph they
have to fire all three full-chapter pipelines and wait. That's wasteful when
they only just rewrote one paragraph and want a fast loop on it.

## Scope
- `src/icons.rs` — add a `PLAY` Font Awesome 4 codepoint constant.
- `src/app/mod.rs` — add `paragraph_play_queue: VecDeque<(String, Pipeline)>`
  to `CkWriterApp`, init empty.
- `src/app/coach.rs` —
  - `pub fn play_paragraph(&mut self, paragraph_id: &str)`: pushes the three
    per-paragraph pipelines (show, prose, spelling) onto the queue and kicks
    off the next run if no stream is in flight.
  - `fn start_single_paragraph_run(&mut self, paragraph_id: &str, pipeline)`:
    builds a one-entry `CoachRun` for the named paragraph, **bypassing the
    dirty-hash cache** — the click is an explicit re-run.
  - Extend `finalize_coach_run` (and the stream-error abort path) to drain
    the queue: pop the next `(paragraph_id, pipeline)` and start it.
- `src/ui/editor.rs` —
  - Detect "hovered paragraph" by pointer Y-band against
    `current_paragraphs` (so the cursor staying in the gutter still counts).
  - For the hovered paragraph only, paint an FA play glyph in the left
    margin, **left of the dirty bar**.
  - Make it clickable (`ui.interact` with a `Sense::click()` rect); click
    calls `app.play_paragraph(id)`.
- Clear `paragraph_play_queue` on chapter switch / book close, so a queued
  paragraph from the previous chapter never fires.

## Out of scope
- Voice pipeline on the play button (it's chapter-level by design).
- A "play all paragraphs" master button — chapter-level pipeline buttons
  already cover that.
- Cancellation / stop button while a play queue is draining.
- Visual progress indicator on the paragraph during playback (the existing
  "stream in flight" status line is enough for v1).
- Changing the cache-respecting behaviour of the chapter-level buttons.

## Acceptance criteria
- [x] Hovering a paragraph in the editor shows a play glyph in the left
  margin, just left of the dirty bar.
- [x] The glyph disappears when the pointer leaves the paragraph's Y-band
  (gutter included so the pointer can land on the icon).
- [x] Clicking the glyph queues show / prose / spelling for that paragraph
  and runs them sequentially.
- [x] A single-paragraph run forces the prompt even if the pipeline's cache
  hash matches the paragraph (force-run is the whole point of the click).
- [x] After a run completes, the gutter colour for that paragraph reflects
  the new state (Clean if no flags, HasIssues if flags landed).
- [x] Switching chapters or closing the book clears any queued paragraphs.
- [x] `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- [x] `cargo test` passes.

## Design notes
- **Force-run vs cache-respecting**: the chapter-level pipeline buttons
  short-circuit when every paragraph's hash already matches the cache. A
  per-paragraph play button with the same semantics would be a no-op on a
  Clean paragraph — surprising. The button is an explicit user action, so
  it always re-fires the prompt. Idempotency is preserved by the existing
  `id_hash` dedupe in `ingest_response` (same paragraph_id + same
  normalized quote → same record id → no duplicate flag piles up).
- **Queue, not three concurrent streams**: `CoachRun` and the stream
  machinery are single-flight by design (`if self.stream.is_some() ||
  self.coach_run.is_some() { return; }`). The queue lets the click feel
  immediate without changing that invariant.
- **Hover detection by Y-band, not glyph rect**: the play glyph sits to the
  left of the prose, in the margin. If we only counted the prose rect as
  "hovered", moving the pointer onto the icon would dismiss it. Y-banding
  the paragraph (top of first line → bottom of last line, full column
  width) keeps the icon visible while the pointer hovers anywhere on its
  row.
- **Glyph placement**: at `output.galley_pos.x - GUTTER_GAP_PX -
  GUTTER_WIDTH_PX - icon_size - small_gap`. Sits inside `MIN_SIDE_PADDING`
  on a tight layout but the icon is small (~12-14 px) so it fits.
