# 0022 — REFACTOR: Build a prose-first editor crate over ropey

**Type:** REFACTOR
**Created:** 2026-05-03
**Depends on:** none
**Supersedes:** the ck-editor fork attempt of this same ticket number (abandoned 2026-05-03 — see "Why not the fork" below).

## Problem
The manuscript pane needs to do four things that the current `egui::TextEdit::multiline` over `editor_text: String` cannot:

1. **Decorations that don't flicker on type** (#0017). Entity highlights, revision underlines, and any future per-paragraph chrome have to share a frame with the typed character — TextEdit's layouter closure runs against last-frame's hit list, so every keystroke past a decorated span strobes for a frame.
2. **Anchors that don't drift** (#0021). Coach cards, revisions, and entity hits need addresses that survive edits made elsewhere in the chapter. Byte offsets into one giant `String` shift every time the user types.
3. **Cheap snapshots for generous undo.** Cloning a string per restore point gets worse as chapters grow.
4. **Paragraph-stable identity.** Per-paragraph caching (#0004), paragraph locks (#0005), paragraph-focused coaching (#0007), three-layer memory (#0008), always-loaded context (#0011) all want a stable `(paragraph_id, beat_id)` cache key. With one `String` everything pays a "find this paragraph in the buffer" tax.

## Approach (new direction)
Build a small, prose-first editor crate inside this repository. Owns its render frame, owns its input, builds on top of `ropey` for the buffer. **Not** a fork of any code editor — written for prose from day one.

### Why a new crate, not a TextEdit
TextEdit can't be made flicker-free without owning the render frame; it doesn't expose a cursor model the host can drive; its layouter closure can never share a frame with the keystroke that produced it. Every decoration we layer on it fights its layout. The flicker is structural.

### Why not the fork
The `~/Projects/CkEditor/ck-editor/` fork attempt (Phase 1–4 of the original draft of this ticket) was tried and abandoned the same day. The crate inherited a code-editor's mental model — modes (Navigate / Edit / Visual / Vim), gutter line numbers, mode indicator, image rendering, markdown reading view — and every prose feature meant either bypassing or aggressively stripping a CkEditor concept. After a full rewrite that ripped vim, modes, gutter, and the markdown view (~3990 net lines deleted), three structural issues remained:
- Click hit-testing under word-wrap was approximate (`pos.x / char_width` ignores wrap, the per-line `screen_y_to_line` lookup was off by `TEXT_LEFT_PAD`).
- Empty-line vs non-empty-line heights drifted by a few pixels each because the height was `(galley.size().y + LINE_PADDING).max(line_height)` and the two galley shapes don't agree.
- Visually it stayed a code editor: monospace, dark `#1c1c20`, 2px line padding, narrow centred column. None of that is prose.

The fork was the wrong substrate. A line-by-line manual layout with `Vec<f32>` line heights reinvents what `egui::Galley` already does — `cursor_from_pos`, `pos_from_cursor`, selection-across-wrapped-rows. The right move is to build the editor at the level egui already provides primitives for, not to hand-roll text layout.

### What the new crate looks like
- **`crates/prose/`** (working name; user picks final). One crate, depended on by the app.
- **Buffer:** `ropey::Rope` + a cursor model (single primary cursor + sticky col, selection anchor head). No `String` source of truth.
- **Document:** paragraph + beat indices over the rope. Paragraphs `\n\n`-delimited, beats `\nl`-token-delimited (the user's LaTeX convention). Stable `(paragraph_id, beat_id)` ids that persist across edits and across save/load via a `.ids.json` sidecar.
- **Render:** one galley per visible viewport (or per visible paragraph — TBD on profiling). Egui's galley primitives handle wrap, hit-test, selection. We paint:
  - Background (paper, off-white per the dyslexia-preference memory)
  - Selection rect via `galley.pos_from_cursor` × 2
  - Decorations (entity highlights, revision underlines) layered into the `LayoutJob` *before* the galley is built — same frame as typing, no flicker by construction
  - The galley
  - Caret via `galley.pos_from_cursor`
- **Input:** one keyboard handler — text inserts, arrows move, Backspace/Delete/Enter, Ctrl+Z/Y/A. No modes. Click → `galley.cursor_from_pos` → buffer cursor. No vim, no Navigate-vs-Edit, no Esc-to-Navigate.
- **Anchors:** `Anchor { paragraph_id, beat_id, char_in_beat }` for everything that wants to refer to a span. Decorations carry anchors, not byte offsets. Edits in paragraph 3 cannot shift anchors in paragraph 7.
- **Undo:** `EditOp` inverses + a stack. Cheap because the rope already supports cheap subset edits.

### What's the relationship to the rest of the app
- `editor_text: String` is replaced by `Document` (rope + index). Reads route through `document.text()` (whole chapter as String, for LLM prompts) or `document.beat_text(anchor)` (paragraph-focused). Existing 50+ read sites get a one-line shim.
- Coach card → cursor: `app.document.set_cursor_to(anchor)`. No bespoke `jump_to_anchor`.
- Save/load: `chapter.tex` (prose, Overleaf-clean) + `chapter.ids.json` (paragraph + beat ids in source order). Missing/stale sidecar reassigns ids in source order on load.

## Out of scope
- **Find/replace UI** — needs the editor first.
- **Per-paragraph caching (#0004), paragraph locks (#0005), paragraph-focused coaching (#0007), three-layer memory (#0008)** — become trivial after this; not folded in.
- **Hover tooltips** — recoverable on top once the galley layer is in place; separate ticket.
- **LaTeX syntax highlighting** — closes #0015; can be added by injecting per-token `TextFormat` into the `LayoutJob` before galley layout. Phase 2 of this ticket, not Phase 1.

## Acceptance criteria
*(Refined when the user starts the work — these are placeholders, not commitments.)*
- [ ] `crates/prose/` exists with `Buffer`, `Document`, `Anchor`, and a `show()` entry point.
- [ ] Manuscript pane renders via the new crate. No `egui::TextEdit::multiline` over `editor_text` anywhere in the manuscript path.
- [ ] Click anywhere in the prose places the cursor exactly on the clicked character, including inside word-wrapped paragraphs and on empty lines.
- [ ] Typing produces no flicker on entity highlights or revision underlines (manual smoke test).
- [ ] All buffer lines have the same visual height when empty or when containing a single short word — no off-by-a-few-pixels drift between empty and non-empty rows.
- [ ] Visual: paper background, prose-friendly font (proportional, dyslexia-considerate per saved memory), generous line spacing (~1.5×), wide-enough column.
- [ ] Multi-paragraph drag-select paints a single continuous selection rectangle across wrapped rows.
- [ ] Cmd+Z / Cmd+Shift+Z restore prior state (text and cursor).
- [ ] `Anchor` ↔ `Position` round-trip; paragraph-id stability across within-paragraph edits.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace` both 0 warnings, 0 errors.

## Lessons captured from the fork attempt
- **Don't fork a code editor for prose.** Modes, gutter, vim, mode indicator, image-buffer, markdown-reading-view were all dead weight that fought every prose change. The "strips" were always going to be most of the crate.
- **Build at egui's primitive level, not below it.** Hand-rolling line-by-line layout, `Vec<f32>` line heights, `pos.x / char_width` hit-test, screen_y_to_line — every one of these is a worse-than-egui re-implementation of `Galley::cursor_from_pos` / `pos_from_cursor`. The galley is the right unit.
- **A `prose_mode` flag is a smell.** If the editor needs to be told it's a prose tool, it's not a prose tool.
- **Visual is half the work.** Even with a flawless render and exact hit-testing, "code editor on a dark page in a centred narrow column with 2px line padding" reads as wrong for prose. The aesthetic is non-negotiable, not a polish pass at the end.
