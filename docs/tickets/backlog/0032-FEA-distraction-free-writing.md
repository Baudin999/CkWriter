# 0032 — FEA: Distraction-free writing mode

**Type:** FEATURE
**Created:** 2026-05-04
**Depends on:** none

## Problem
While drafting, every visual element competes with the prose: the file tree, top bar, scope/inspector panels, the per-paragraph dirty gutter (#0023), revision underlines, entity highlights, paragraph play button (#0024), author-note marks (#0027). Each one is useful in its own context, but in a discovery-writing flow they add up to a noisy page that pulls the eye away from the words. The writer wants a single toggle that strips the screen down to nothing but the editor column and the prose itself, keeping only LaTeX command highlighting (#0015) because that is part of the text the writer is actively shaping, not feedback layered on top of it.

## Scope
- Add an app-level `distraction_free: bool` flag (off by default) and a keybinding to toggle it. Persist across sessions in `Settings`.
- When on:
  - Hide all chrome around the editor: file tree, top bar, scope panel, inspector, status bar, any side panels. The editor column fills the window.
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
- [ ] `Settings` gains a `distraction_free: bool` (default false), persisted in `settings.toml`.
- [ ] A keybinding toggles the flag live (no restart). Bind to a Super-based shortcut consistent with the rest of the app; verify it does not collide with existing bindings or with Hyprland in `~/.config/hypr/`.
- [ ] With the flag on, the file tree, top bar, scope panel, inspector, and status bar are not rendered; the editor column fills the available area.
- [ ] With the flag on, the per-paragraph dirty gutter, revision underlines + chips, entity highlights, paragraph play button, and author-note marks are all suppressed in the editor.
- [ ] With the flag on, LaTeX command tokens still render in `theme::LATEX_COMMAND` (verify against a paragraph containing `\emph{...}`, `\nl`, `\switch`).
- [ ] Toggling off restores every hidden element exactly as before — no layout flicker, no lost caret, no double-paint.
- [ ] Coach pipelines and dirty tracking keep running while the flag is on (assert by toggling off after editing and confirming the gutter / revisions appear as expected).
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- The cleanest seam is a single `bool` read inside each painter: gutter painter, revision-layer builder, entity-layer builder, play-button painter, author-note painter, and the chrome panel composers in `src/app/mod.rs` / `src/ui/*`. Each one early-returns or skips its layer when the flag is on. Avoid a parallel "distraction-free renderer" — that would drift.
- LaTeX layers come from `latex_layers(...)` in `src/ui/editor.rs`; that path stays unconditional. Revision and entity layers are the ones to gate.
- Keep the toggle a pure view flag. Do NOT pause or cancel coach runs when entering the mode — the writer expects to leave the mode and find their feedback waiting.
- A Super-based key is required (omarchy/Hyprland; never Cmd, avoid Ctrl). Suggest Super+. or Super+Shift+Z, but pick whatever is unbound after grepping the existing shortcut table and the Hyprland config.
- This is not a theme — leave #0030 alone. A user in light theme + distraction-free still gets the cream page; a user in dark theme still gets the dark page.
