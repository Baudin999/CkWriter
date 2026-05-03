# 0023 — FEA: Per-paragraph dirty gutter in the editor

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0002, #0004

## Problem
With #0004 in, the writer can re-run a coach pipeline cheaply — but they have no visual signal of *which* paragraphs are dirty until the run starts and the K/N counter ticks. They want to know, while editing, "if I ran prose right now, which paragraphs would the model see?" — and ideally without having to read a status line in the side panel. The natural place to put that signal is the editor margin, the way every code editor shows git-diff bars.

## Scope
- A thin left-margin gutter (~6–10 px) attached to the LaTeX editor in `src/ui/editor.rs`. Lives inside the same horizontal layout as the TextEdit so it scrolls with the buffer.
- For every paragraph that has at least one cache miss across **show / prose / spelling**, paint a vertical line in the gutter spanning that paragraph's vertical extent in the rendered galley. Voice is excluded (chapter-level, not per-paragraph).
- "Dirty" is computed against `current_paragraphs` (the saved index) and `current_chapter.meta.last_run_hashes` (the cache from #0004). A paragraph is dirty if any of `{show, prose, spelling}` either lacks a cache entry for that paragraph_id or has a cache entry whose hash differs from the paragraph's current hash.
- Paragraph-to-pixel conversion uses the `output.galley.pos_from_ccursor` path the editor already uses for `scroll_to_cursor` (see `src/ui/editor.rs:198`). The gutter draws after TextEdit lays out, so wrap-aware positions are available.
- Update on every frame the editor renders — cheap because the dirty set is N≪chapter and `pos_from_ccursor` is already hot-pathed.
- Theme-friendly color: a single muted accent (something like `theme::TEXT_MUTED` or a new `theme::GUTTER_DIRTY`). One color, binary signal.

## Out of scope
- Per-pipeline color coding (three side-by-side dots / thirds of a line). v2 if the single-line indicator turns out to be too coarse.
- Click-to-jump from the gutter line to the paragraph. Nice-to-have, not required for v1.
- Live indication during typing for paragraphs that haven't been saved yet. `current_paragraphs` only refreshes on chapter open and on save — the gutter therefore lags live edits until the next save. That's deliberate: the writer gets a clean "since-save" signal, not a flickering per-keystroke one.
- Line numbers, fold markers, or any other gutter content. Keep the gutter single-purpose.
- Showing dirty state for any pipeline outside coach (e.g. character-extraction queue, progression). Those are different concerns.

## Acceptance criteria
- [ ] On a freshly-opened chapter where no coach has ever run, every paragraph shows a gutter line.
- [ ] After running `prose` to completion, lines for prose-cached paragraphs disappear *only* if `show` and `spelling` are also cached for them; otherwise the line stays (because the union across the three pipelines is still dirty).
- [ ] After running all three of `show`, `prose`, `spelling` to completion on a clean chapter, no gutter lines are visible.
- [ ] Editing a paragraph and saving brings its gutter line back; the paragraphs around it stay clean.
- [ ] Running `voice` (chapter-level) does not change any gutter line.
- [ ] The gutter scrolls with the editor — line positions track the rendered galley, not the unwrapped text.
- [ ] `cargo clippy --all-targets -- -D warnings` clean; `cargo test` clean.

## Design notes
- **Why aggregate, not per-pipeline:** the writer asked for one line. A second display surface (e.g. three colored dots) trades calm for information density we haven't proven we need. Per-pipeline counts can still live in the AI panel as a separate v2 if the aggregate gutter feels too coarse.
- **Why "since-save" semantics, not "since-keystroke":** `current_paragraphs` is the authoritative paragraph index and only refreshes on save. Recomputing it every keystroke would either duplicate the splitter work in two paths or shift the splitter onto the per-frame editor render. Either is more change than this ticket warrants. The lag is also unlikely to bother the writer — the gutter answers "what will the next coach run cost?", which is a save-time question, not a keystroke question.
- **Where in editor.rs:** wrap the existing `TextEdit::multiline(...)` block in a `ui.horizontal` so a fixed-width gutter strip sits to its left. The strip allocates the same height as the rendered galley and uses `ui.painter()` to draw `Shape::line_segment` per dirty paragraph. Pixel positions come from `output.galley.pos_from_ccursor(CCursor::new(byte_offset))` for the start and end of `Paragraph::char_range`.
- **Performance:** dirty-set computation is O(paragraphs × pipelines) = a few hundred hash lookups at worst. No need to memoize — recomputing per frame is fine. If profiling later proves otherwise, cache the dirty set keyed on `(editor_text_hash, last_run_hashes_revision_counter)`.
- **Gutter width:** start at 8 px. Easy to tune later.

## Verification (smoke test)
1. Open a chapter that has never been coached — confirm a gutter line beside every paragraph.
2. Run `prose`. Confirm gutter lines are unchanged (because show/spelling are still uncached for those paragraphs).
3. Run `show` and `spelling`. Confirm gutter clears for unchanged paragraphs.
4. Edit one paragraph and save. Confirm only that paragraph's gutter line returns.
5. Run `voice`. Confirm gutter is unaffected.
