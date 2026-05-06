# 0032 — FEA: Distraction-free writing mode

**Type:** FEATURE
**Created:** 2026-05-04
**Depends on:** none

## Problem
While drafting, every visual element competes with the prose: the file tree, top bar, scope/inspector panels, the per-paragraph dirty gutter (#0023), revision underlines, entity highlights, paragraph play button (#0024), author-note marks (#0027). Each one is useful in its own context, but in a discovery-writing flow they add up to a noisy page that pulls the eye away from the words. The writer wants a single toggle that strips the screen down to nothing but the editor column and the prose itself, keeping only LaTeX command highlighting (#0015) because that is part of the text the writer is actively shaping, not feedback layered on top of it.

## Scope
- Add an app-level `distraction_free: bool` flag (off by default) and a **floating corner toggle button** as the sole entry/exit point. Persist across sessions in `Settings`. **No keybinding** — the button is always painted (more muted in distraction-free mode) so it's the one piece of chrome that survives the toggle.
- When on:
  - Hide all chrome around the editor: file tree, top bar, scope panel, inspector, status bar, any side panels. The editor column fills the window. The floating toggle button stays painted (corner-pinned `egui::Area`).
  - Hide all editor decorations *except* LaTeX command highlighting (#0015):
    - per-paragraph dirty gutter (#0023) — no bar painted
    - revision underlines (voice / show-don't-tell / prose / spelling / punctuation / grammar) — no underline, no chip
    - entity highlights — text renders at the default foreground, no recolor, no underline
    - paragraph play button (#0024) — not painted
    - author-note marks (#0027) — not painted
    - hover-only gutter glyphs — suppressed
  - LaTeX command tokens still render in `theme::LATEX_COMMAND` exactly as in normal mode.
- When off, every hidden element returns to its normal state with no residue (no leftover layout shift, no lost selection, no pipeline state changes).
- Toggle is purely a *view* switch: coach pipelines, dirty tracking, revisions, and entity scanning all keep running in the background. Turning the mode off shows whatever has accumulated.

## Out of scope
- A typewriter/centered-line mode (always-center the active line). File as a follow-up if wanted.
- Dimming surrounding paragraphs ("focus mode" on the current paragraph only). Different feature.
- Hiding the cursor, scrollbars, or window decorations. The OS/window stays as-is.
- Per-decoration toggles ("hide revisions but keep entities"). One switch, all-or-nothing, by design — resist building a framework.
- A separate "presentation" theme or font swap. Theming belongs to #0030.
- Auto-enter on idle / auto-exit on coach completion. Manual toggle only.

## Acceptance criteria
- [x] `Settings` gains a `distraction_free: bool` (default false), persisted in `settings.toml`.
- [x] A floating corner button toggles the flag live (no restart). Painted as an `egui::Area` anchored to `RIGHT_TOP`, frameless with `theme::TEXT_MUTED`, so it survives the chrome being hidden and stays a hint rather than a distraction.
- [x] With the flag on, the file tree, top bar, scope panel, inspector, and status bar are not rendered; the editor column fills the available area. The floating toggle button is the only painted UI element outside the editor column.
- [x] With the flag on, the per-paragraph dirty gutter, revision underlines + chips, entity highlights, and paragraph play button are all suppressed in the editor. (Author-note marks have no painter yet — when #0027's painter lands it must read the same flag.)
- [x] With the flag on, LaTeX command tokens still render in `theme::LATEX_COMMAND` — `latex_layers(...)` runs unconditionally and italic_ranges still apply to the text format.
- [x] Toggling off restores every hidden element exactly as before — pure view flag, no state changes, no caret loss.
- [x] Coach pipelines and dirty tracking keep running while the flag is on. The flag zeroes `entity_hits` / `revisions` at the editor-render call site only; `app.entity_hits` and `app.revisions` keep updating in the background.
- [x] `cargo clippy --all-targets -- -D warnings` and `cargo test` clean (0 warnings, 0 errors). Verified: 246 tests passing.

## Design notes
- The cleanest seam is a single `bool` read inside each painter: gutter painter, revision-layer builder, entity-layer builder, play-button painter, author-note painter, and the chrome panel composers in `src/app/mod.rs` / `src/ui/*`. Each one early-returns or skips its layer when the flag is on. Avoid a parallel "distraction-free renderer" — that would drift.
- LaTeX layers come from `latex_layers(...)` in `src/ui/editor.rs`; that path stays unconditional. Revision and entity layers are the ones to gate.
- Keep the toggle a pure view flag. Do NOT pause or cancel coach runs when entering the mode — the writer expects to leave the mode and find their feedback waiting.
- The floating button is the only entry/exit point. Paint it via `egui::Area::new(...).fixed_pos(...)` so it floats above whatever panel layout is active. Place it top-right with a small inset; in distraction-free mode the button uses `theme::TEXT_MUTED` with `Sense::click` only (no frame); in normal mode it can use the standard egui button style.
- This is not a theme — leave #0030 alone. A user in light theme + distraction-free still gets the cream page; a user in dark theme still gets the dark page.
