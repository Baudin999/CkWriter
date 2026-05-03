# 0022 — REFACTOR: Fork ck-editor as the manuscript editor with paragraph/beat addressing

**Type:** REFACTOR
**Created:** 2026-05-03
**Depends on:** none

## Problem
The manuscript editor renders the entire chapter as **one** `egui::TextEdit::multiline` over a single `String` (`src/app/mod.rs:76`, `src/ui/editor.rs:170-178`). Every keystroke triggers a global re-layout, with all decorations recomputed against absolute byte offsets into one giant buffer. This is the wrong shape for a writing tool, and it is the **structural** cause of two ongoing bug classes:

1. **Highlight flicker on type (#0017).** The pre-render `refresh_entity_hits` gate (`editor.rs:61-64`) runs at the top of `show()` *before* the keystroke for that frame has been applied — egui only mutates `editor_text` inside `edit.show(ui)`. The layouter therefore receives the new text against a closure-captured snapshot of the pre-keystroke hits. The strobe described in #0017 is one frame late, not eliminated. Fundamentally: when egui owns the buffer and the render frame, decorations cannot stay in sync with typing.
2. **Anchor drift / off-by-N positioning (#0021).** Decorations carry absolute `(byte_start, byte_end)` into `editor_text`. Any edit above an anchored span shifts every span below it. The whole document is one address space; any edit far from a coach anchor still moves it.

Beyond bugs, the single-buffer model blocks several near-term initiatives:
- **Per-paragraph caching (#0004)**, **paragraph locks (#0005)**, **paragraph-focused coaching (#0007)**, **three-layer memory (#0008)**, **always-loaded context (#0011)** — all want paragraph-stable identity for cache keys, lock state, prompt scoping, recall granularity.
- **Generous undo** requires cheap snapshots; cloning a `String` per restore point gets worse as chapters grow.
- **Coach-card → editor navigation** (`jump_to_anchor`) reinvents what a real editor's cursor/selection model gives for free, and is the proximate site of #0021.

The cost of *not* doing this: every new paragraph-scoped feature pays a "first, find this paragraph in `editor_text` and translate offsets" tax; #0017-class flicker bugs continue to surface in new decoration layers (#0015, #0020); coach navigation keeps drifting; undo gets shoe-horned in on top of a buffer that doesn't structurally support it.

### Why a forked `ck-editor`, not `egui::TextEdit`
`egui::TextEdit::multiline` requires `&mut String`, doesn't own its render frame, doesn't expose a cursor model we can drive from outside. Every decoration we layer on top fights its layout closure. The flicker is structural to that arrangement, not a bug we can patch.

`~/Projects/CkEditor/ck-editor/` already solves these problems: it owns the buffer (`Buffer` over `ropey::Rope`), the cursor model (`CursorState` with anchor+head and sticky col), the input loop (mouse hit-test, drag-select, scroll), the render frame (viewport-clipped line rendering with selection + highlights composited atomically inside one frame), and undo (`EditOp` inverses + `UndoStack`). It is feature-complete for everything CkWriter's manuscript pane needs.

The right move is to **fork** it: copy `ck-editor/`, `ck-types/`, and `ck-markdown/` into CkWriter as `crates/editor/`, `crates/types/`, `crates/markdown/`; strip the few code-editor-specific bits we don't want; add a thin paragraph/beat index over the existing rope. This is less code than rebuilding equivalent capability over `egui::TextEdit`, and inherits a working flicker-free render path on day one.

### Why paragraph + beat addressing
Paragraphs are `\n\n`-separated. Beats are delimited by the user's custom LaTeX `\nl` token (newline-with-indent, no top-margin) — a literal token in the source, not a whitespace character. Decorations and coach anchors stored as `(paragraph_id, beat_id, char_in_beat)` are structurally bounded against drift: an edit in paragraph 3 cannot shift paragraph 7's anchors. `paragraph_id` and `beat_id` are stable across edits within a session and persisted on disk, so per-paragraph caches (#0004), locks (#0005), and coaching context (#0007, #0008) have stable keys.

`(paragraph_id, beat_id)` is a *logical* index over the rope, not a replacement for `Position{line, col}`. A beat may span multiple physical lines; a single physical line may contain multiple beats. The cursor stays `(line, col)`-keyed (provided by the forked editor); the decoration/anchor model is paragraph/beat-keyed (provided by `Document`); resolution between the two is a function of the index.

## Approach
Fork `~/Projects/CkEditor/{ck-editor,ck-types,ck-markdown}/` into CkWriter, adapt for prose, add a paragraph/beat index over the existing `(line, col)` model.

### Workspace conversion + copy
1. Convert `~/Projects/CkWriter/Cargo.toml` into a hybrid `[workspace]` + `[package]` manifest. Root remains the app crate; add `members = ["crates/editor", "crates/types", "crates/markdown"]`.
2. `cp -r` from CkEditor:
   - `~/Projects/CkEditor/ck-editor/` → `~/Projects/CkWriter/crates/editor/`
   - `~/Projects/CkEditor/ck-types/` → `~/Projects/CkWriter/crates/types/`
   - `~/Projects/CkEditor/ck-markdown/` → `~/Projects/CkWriter/crates/markdown/`
3. The copied crates' `path = "../ck-types"` / `path = "../ck-markdown"` references already work — they're now siblings under `crates/`.
4. Add `editor = { path = "crates/editor" }` to CkWriter's main `[dependencies]`.
5. Build passes on first try (modulo workspace-resolver bumps).

### Strips
- `ck-types` dependency from `ck-editor` — replaced with a minimal in-crate types module. CkWriter has no LSP. (`ck-types` itself stays in `crates/types/` only if `ck-markdown` needs it; otherwise drop the crate entirely.)
- `Buffer::apply_lsp_edits` and its tests — same reason.

That is the entire strip list. Vim, syntect, ck-markdown reading view, gutter, image, search, mode handlers, undo all stay.

### Adds
- `crates/editor/src/document.rs` — owns paragraph/beat indices over the rope. Public surface:
  - `paragraphs() -> &[ParagraphRef]` and `beats(paragraph_id) -> &[BeatRef]` — derived views.
  - `position_of(anchor: Anchor) -> Option<Position>` — resolve `(paragraph_id, beat_id, char_in_beat)` → `(line, col)`. Returns `None` if the id has been retired.
  - `anchor_of(position: Position) -> Anchor` — inverse, used when storing a new decoration from a cursor selection.
  - `rebuild_indices()` — called after structural rope edits (paragraph split/join, beat insert/remove). Within-beat character edits don't trigger a rebuild.
- Decoration data structures keyed by `Anchor`, not byte offsets:
  - `EntityHit { anchor: Anchor, char_len: usize, ... }`
  - `Revision { anchor: Anchor, ... }`. `SuggestionRecord` rebuild path uses `anchor_of` against the quote-text match.
- Render-side decoration pass that overlays entity-hit colors and revision underlines on the existing line render, layered after syntect tokens, before selection rectangles.

### Rewires in CkWriter
- `CkWriterApp::editor_text: String` (`src/app/mod.rs:76`) → the new editor's `Buffer`. Single source of truth.
- `CkWriterApp::current_paragraphs: Vec<Paragraph>` (`src/app/mod.rs:87`) → derived view via `Document::paragraphs()`. Eliminated as standalone state.
- `src/ui/editor.rs` — replaced. Today's whole file becomes a thin host that calls into the new editor's `show()`.
- `src/app/book.rs` — `open_chapter`, `delete_chapter`, `resync_current_chapter`, `refresh_entity_hits` become editor-aware. `jump_to_anchor` is **deleted**; every call site is replaced by `buffer.cursor.set_primary(...)`.
- `src/app/coach.rs` — ingest (`coach.rs:234-238`), rebuild (`coach.rs:407-466`), auto-stale all switch to `Anchor` storage and resolve via `Document` at render time.
- `src/extract.rs::EntityMatcher::find` — pure function, unchanged. Call sites pass per-beat or per-paragraph slices instead of the whole chapter.
- LLM prompts — `document.text()` (whole rope as `String`) for chat/coach pipelines that want the whole chapter; `document.beat_text(anchor)` for paragraph-focused coaching (#0007 lands cleanly on this).
- Persistence — chapter file format extends to carry `paragraph_id` and `beat_id`. See design notes.

### Coach card → cursor (closes #0021)
Click on a coach card resolves the stored `Anchor` to a `Position` range, then:
```rust
buffer.cursor.set_primary(Cursor { anchor: start_pos, head: end_pos });
```
The forked editor's existing render scrolls the cursor into view and paints the selection rectangle. No bespoke `jump_to_anchor`. Anchor-drift bug class collapses: `Anchor` is paragraph/beat-relative, so only `char_in_beat` can drift, and only within that beat.

### Multi-paragraph selection with autoscroll
Already supported by the forked editor — `view::input::handle_mouse` does drag-select with `Sense::click_and_drag`, and the existing scroll-wheel logic plus drag-past-viewport handling cover autoscroll. Nothing to add for this ticket.

### Undo / redo
Inherited as-is via `Buffer::undo` / `Buffer::redo` and `UndoStack`. Cmd+Z / Cmd+Shift+Z bound in the manuscript pane. No separate ticket needed.

### Toggles for ported features
- **Vim mode:** `vim_mode = false` for the manuscript pane. Code stays in the crate; users who want vim toggle in settings.
- **Syntect:** enabled for `.tex` chapters (closes #0015 LaTeX command highlighting), bypassed for plain prose chapters.
- **`ck-markdown` reading view:** kept for non-chapter content (character bios, world-building notes, scratch docs). Manuscript chapters bypass it.
- **Image inline:** kept; manuscript chapters can include image references that render inline.
- **Gutter:** kept; markers retyped for prose-domain concerns (paragraph lock state #0005, cache state #0004, coach flag, unaccepted revision count, word count). Day-one delivery is the unaccepted-revision marker; the rest land with their respective tickets but the gutter pipeline is wired and ready.
- **Search:** kept as-is for find within chapter; writer-facing find/replace dialog is a follow-up ticket.

## Out of scope
- **Find/replace UI.** The forked editor exposes search primitives; the writer-facing dialog is its own ticket.
- **Per-paragraph caching (#0004), paragraph locks (#0005), paragraph-focused coaching (#0007), three-layer memory (#0008).** These become *easier* after this lands; not folded in.
- **Sentence-level / environment-aware paragraph splitting.** Paragraphs stay `\n\n`-delimited, beats stay `\nl`-delimited, exactly as the chapter source today.
- **Cross-project shared editor crate.** If a third project ever wants the editor, extract `crates/editor/` to a shared crate then. For now: in-tree fork, no shared cadence with CkEditor.
- **Migrating chat / settings / form text inputs to the new editor.** Single-line fields stay `String` + `egui::TextEdit::singleline`.

## Acceptance criteria
- [ ] `crates/editor/`, `crates/types/`, `crates/markdown/` exist in CkWriter, originated from the corresponding `~/Projects/CkEditor/` crates. Root `Cargo.toml` is a hybrid workspace + package manifest.
- [ ] `ck-types` usage inside `ck-editor` is removed; `Buffer::apply_lsp_edits` is deleted. No other strips applied.
- [ ] `editor_text: String` and `current_paragraphs: Vec<Paragraph>` are gone from `CkWriterApp`. `Document` (rope + paragraph/beat indices) is the single source of truth for chapter text and structure.
- [ ] Manuscript pane renders via the forked editor's `show()`. Typing produces no visible flicker on entity highlights or revision underlines (manual smoke test on a chapter with several of each, typing inside and adjacent to highlighted spans).
- [ ] `jump_to_anchor` is deleted. Every coach-card click, search-result navigation, and "go to here" path sets `buffer.cursor.set_primary(...)` directly.
- [ ] Anchor-drift regression test: edit paragraph 3 of a multi-paragraph chapter; assert revision anchors in paragraph 7 still resolve to the same substring before and after.
- [ ] Multi-paragraph selection: click-drag from paragraph 2 into paragraph 6 selects the whole intervening range; dragging past the viewport edge autoscrolls. Cmd+A selects the whole chapter.
- [ ] Cmd+Z / Cmd+Shift+Z restore prior buffer state including cursor position. Multiple undo steps work correctly across both within-paragraph edits and structural mutations (paragraph split/join).
- [ ] Coach card → click → cursor lands on the suggested span, viewport scrolls to bring it into view, selection rectangle paints the suggested range.
- [ ] Save/load round-trips chapter content for an arbitrary input. Each chapter is paired with a `.ids.json` sidecar; loading a `.tex` without (or out of sync with) its sidecar reassigns ids in source order and rewrites the sidecar on next save.
- [ ] Paragraph and beat ids are stable across edits within a session and across save/load cycles.
- [ ] LaTeX chapters (`.tex`) get syntect token highlighting (closes #0015). Plain prose chapters bypass syntect.
- [ ] Inline images render where they appear in the source (kept feature from `ck-editor::view::image`).
- [ ] Gutter shows at minimum the unaccepted-revision marker per paragraph; pipeline is wired for additional marker kinds to be added by future tickets.
- [ ] LLM pipelines (coach, chat, character extraction) receive identical input strings to today: prompt assembly is invisible to the migration.
- [ ] `cargo clippy --all-targets -- -D warnings` returns 0 warnings, 0 errors. `cargo test` returns 0 failures. New tests cover: `Document` paragraph/beat index correctness against `\nl`-tokenized fixtures, `Anchor` ↔ `Position` round-trip, paragraph-id stability across edits, coach-card cursor resolution, save/load round-trip including ids.

## Design notes

### Beats are `\nl`-delimited, not `\n`-delimited
The user's chapter source uses a custom LaTeX command `\nl` for "newline with indent, no top-margin." Beats are tokenized by scanning the rope for the literal token `\nl`. Implications:
- A single physical line in the rope can contain multiple beats (e.g. `"He paused. \nl She looked up."` — one line, two beats).
- A single beat can span multiple physical lines (LaTeX line wrapping in source).
- `(paragraph_id, beat_id)` is **not** a relabeling of `(line, col)`. They are two separate logical indices over the same rope.

The beat tokenizer is part of `Document`. It must handle escaped occurrences (`\\nl` is a literal backslash-n-l, not a beat boundary) and occurrences inside LaTeX comments (`% ... \nl ...` should not split). Tests cover both cases.

### Two indices, one rope
- **Physical (`Position{line, col}`):** the cursor position, mouse hit-test result, viewport clipping unit. Provided by the forked editor's existing `Buffer`/`CursorState`.
- **Logical (`Anchor{paragraph_id, beat_id, char_in_beat}`):** decoration storage, coach anchors, paragraph/beat-scoped LLM context. Provided by `Document`.
- **Rope:** the single source of truth both indices derive from.

`Document::rebuild_indices()` is called whenever the rope mutates structurally (paragraph split via Enter at end of paragraph, beat insert via typed `\nl`, paragraph join via Backspace at start of paragraph, structural undo). Within-beat character edits don't trigger a rebuild — the index's top-level structure is unchanged.

### Coach card click is just cursor placement
Today: `jump_to_anchor` parses byte offsets, scrolls manually, paints a bespoke highlight that has its own drift problems. Tomorrow: `buffer.cursor.set_primary(Cursor { anchor, head })`. The editor's existing render scrolls to the cursor and paints the selection. The "highlight" the writer sees is just the standard selection rectangle, which means it survives any rope edit, follows the cursor as the writer types, and matches every other navigation primitive (search results, future "go to definition"-style jumps). One navigation API, one rendering path, no drift.

### Paragraph and beat identity scheme
- Splitting paragraph 5 into 5 + new tail: paragraph 5 keeps its id; the new tail gets a fresh id from `Document::next_paragraph_id`.
- Joining (Backspace at start of paragraph): right paragraph merges into left; left's id wins; right's id is retired.
- Beats follow the same pattern at the beat level under their parent paragraph.
- Save/load: ids are persisted alongside paragraph/beat text. Files written before this ticket lack ids; load reassigns in source order.

### File format change — sidecar JSON
The current chapter file is plain LaTeX, paragraphs separated by blank lines, beats separated by `\nl`. Ids are persisted in a **sidecar JSON file** paired by base name:

```
chapter01.tex          ← prose, untouched, Overleaf-readable
chapter01.ids.json     ← [{paragraph_id, beats: [beat_id, ...]}, ...] in source order
```

**Save:** write both files. The `.tex` is the writer's prose; the `.json` is regenerated from `Document`'s current id state.

**Load:** read both. Zip the ids onto the parsed paragraphs/beats. If the `.json` is missing, malformed, or out of sync with the `.tex` (e.g. someone added a paragraph in Overleaf), reassign ids in source order for the unmatched parts and write a fresh `.json` on next save. Stale ids are never an error — they're just regenerated.

The `.tex` stays a clean LaTeX document; Overleaf never sees CkWriter metadata; prose diffs don't churn with id changes; re-importing externally edited `.tex` is the normal load path, not a special case.

### Why everything ports across (vim, syntect, markdown, image, gutter)
Earlier framing dismissed these as "code-editor-specific." That is the wrong split. The right split is **primitive vs content**: cursor/selection/gutter/image/viewport/render are domain-neutral primitives; only the *types of markers* and the *addressing scheme* are domain-specific. CkWriter benefits from every primitive in the forked crate; we only swap what the markers *mean* and add a logical addressing layer on top.

### Bridging `egui` and the rope
Already solved by the forked editor. The CkWriter manuscript pane gets `&mut Document` (which holds `Buffer`); each frame calls `editor.show(ui, theme)`; the editor handles input → buffer mutations → rope → `rebuild_indices` (only on structural edits) → render. No `materialised: String` shuttle needed — that whole problem was specific to the per-paragraph-`TextEdit` design earlier drafts of this ticket proposed and we abandoned.

### What we're betting on
- That `ck-editor`'s rope+cursor+render+undo machinery, which works in CkEditor's code-editor context, works equally well as the substrate for prose. The render is line-by-line over a rope; prose is line-by-line over a rope. There is no domain-specific assumption in the render path that breaks for prose.
- That stripping is small (`ck-types` usage + `apply_lsp_edits`) and adding is contained (`document.rs` + decoration overlay pass + a few rewires).
- That paragraph/beat as a logical index over `(line, col)` is the right separation. If profiling later shows the index rebuild is hot, we optimize incrementally — there is no architectural lock-in.

### Ordering hint for the implementer (not a sub-ticket boundary)
Each step keeps the build green; each step delivers something visible.
1. **Workspace + copy.** Convert root `Cargo.toml` to hybrid workspace + package. `cp -r` the three CkEditor crates. Build passes; CkWriter app does not yet use the new editor.
2. **Strip.** Remove `ck-types` usage from `ck-editor` (inline a minimal types module if needed). Delete `Buffer::apply_lsp_edits` and tests. Build passes.
3. **Add `Document`.** Implement paragraph/beat indices over the rope. Unit tests against `\nl`-tokenized fixtures including escape and comment edge cases.
4. **Swap manuscript pane.** Wire CkWriter to render via the new editor. `editor_text: String` and `current_paragraphs: Vec<Paragraph>` deleted. Decorations still keyed by absolute byte offsets at this step — flicker fix arrives here even though anchor drift remains.
5. **Migrate decorations to `Anchor`.** `EntityHit` and `Revision` switch to `(paragraph_id, beat_id, char_in_beat)`. Render-side overlay pass added. Anchor-drift fix arrives here.
6. **Replace `jump_to_anchor`.** Delete the bespoke navigation. Every call site routes through `buffer.cursor.set_primary(...)`. Coach card → cursor lands here.
7. **Persist ids.** Sidecar `.ids.json` written alongside each `.tex`. Load path tolerates missing/stale sidecars by reassigning in source order.
