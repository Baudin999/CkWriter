# 0016 — BUG: Editor multi-click selection (word / line / paragraph)

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
In the manuscript editor (`src/ui/editor.rs`), the underlying `egui::TextEdit` only treats single-click as cursor-place and double-click as select-word. There is no triple- or quadruple-click selection. The writer expects the standard desktop-editor convention:

- 1 click  → place cursor
- 2 clicks → select word
- 3 clicks → select line
- 4 clicks → select paragraph (where "paragraph" matches our paragraph splitter — bounded by blank lines OR by `\nl`)

Without this, every "select this paragraph to retype" operation is a manual drag, which is slow and error-prone over long blocks. The fact that our `\nl` is *also* a paragraph break (not just a blank line) means we can't lean on whatever upstream `TextEdit` offers even if a future version adds triple-click — it would split on the wrong boundary.

## Scope
- Detect double-, triple-, and quadruple-click on the editor `TextEdit` response, using egui's pointer-click-count over a short time window (default 400 ms between clicks, like most desktop editors).
- 2 clicks: select the word containing the cursor (this matches `TextEdit`'s default — keep the default behavior; just ensure our handler doesn't overwrite it).
- 3 clicks: select the line (the run of characters between the previous `\n` and the next `\n` or end-of-text, exclusive of the newlines).
- 4 clicks: select the paragraph, where a paragraph is the maximal run bounded by:
  - a preceding blank line (or start-of-text), AND
  - a following blank line (or end-of-text), OR
  - a `\nl` token on either side (treat `\nl` as an explicit paragraph break — the selection ends just before `\nl` and the next paragraph starts just after it).
- The selection is set on the `TextEdit`'s state via egui's text-cursor APIs.

## Out of scope
- Re-using the paragraph splitter from `src/book/paragraphs.rs` directly. That splitter operates on byte ranges of `editor_text` for indexing purposes; the multi-click selection needs lightweight char-range logic over the live buffer. They can share helpers later if it's cheap, but the priority is correct selection in the editor, not refactoring.
- Multi-click selection in the chat input, inspector text fields, or any other `TextEdit`. Scope is the manuscript editor only.
- Selection on touch / stylus.

## Acceptance criteria
- [ ] One click places the cursor at the click position (existing behavior preserved).
- [ ] Two quick clicks select the word under the cursor (existing behavior preserved).
- [ ] Three quick clicks select the entire line (between the two surrounding `\n`s, or the start/end of buffer).
- [ ] Four quick clicks select the entire paragraph, with the paragraph boundary being either a blank line OR a `\nl` token on each side.
- [ ] A `\nl` mid-paragraph correctly splits selection: 4-clicking before the `\nl` selects only up to (not including) the `\nl`; 4-clicking after selects only from after the `\nl`.
- [ ] Click count resets after 400 ms of pointer idle, so a slow second click is treated as a fresh single click.
- [ ] Clicks at different positions don't accumulate — the click count only advances when the pointer is at the same character position (or within a few pixels) as the previous click.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- egui exposes `Response::clicked()` and `ctx.input(|i| i.pointer.button_clicked())` and friends, but does not natively expose the click count past 2. Maintain a tiny per-frame state in `App` (or `egui::Memory` keyed on the editor `Id`): `(last_click_at, last_click_pos, click_count)`. Increment when within 400 ms and ~3 px of the prior click; reset otherwise.
- "Line" boundaries are pure `\n` scans on the visible buffer, so they are independent of LaTeX or our paragraph splitter.
- "Paragraph" boundaries: scan backward from cursor for the nearest `\n\s*\n` or `\nl` token; scan forward similarly. Exclude the boundary tokens themselves from the selection.
- Apply the selection via `egui::TextEdit`'s `cursor_range` state (set the primary and secondary cursors).
