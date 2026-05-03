# 0028 — FEA: Super+I wraps selection in `\emph{...}`

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** none

## Problem
Italic emphasis is the writer's most-used inline LaTeX wrap. Today it costs five characters and a cursor reposition for every emphasis: type `\emph{`, type the word, type `}`. Mid-sentence the friction is enough to skip the emphasis or break flow — and skipped emphasis weakens the prose.

## Scope
- **Super+I with a non-empty selection**: replace the selection with `\emph{<selection>}`. Cursor lands just after the closing `}`.
- **Super+I with no selection**: insert `\emph{}` at the cursor and place the cursor between the braces, ready to type.
- Works in any paragraph; locked paragraphs (#0005) suppress coach generation, not typing, so wrapping is unaffected.
- Keybinding registered through whatever editor-shortcut plumbing the project already uses (match the existing pattern, don't invent a new one).

## Out of scope
- Bold (`\textbf`) or other wraps. Writer explicitly does not want them in v1.
- Toggling: Super+I when the cursor is already inside `\emph{…}` does NOT unwrap. Re-file as a follow-up if the writer asks.
- Configurable wrap commands. One keybinding, one command. Resist building a framework.
- Smart whitespace handling (e.g. moving leading space outside the wrap). v1 wraps exactly the selection.

## Acceptance criteria
- [ ] Selecting `the rope` and pressing Super+I produces `\emph{the rope}` with the cursor positioned after the `}`.
- [ ] With no selection, pressing Super+I inserts `\emph{}` and places the cursor between the braces.
- [ ] Selection containing existing LaTeX (e.g. `\emph{x}`) wraps verbatim (`\emph{\emph{x}}`); we do not try to be clever.
- [ ] Multi-line selection wraps the whole span without losing line breaks.
- [ ] Existing editor shortcuts still work — Super+I doesn't shadow any current binding.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- User runs Arch with omarchy (Hyprland), so Super is the natural modifier; do not use Cmd or Ctrl.
- After wrapping, fire whatever the editor's "text changed" path normally fires so the LaTeX highlighter (#0015) and downstream coach machinery see the new state.
- If the editor's input system doesn't already expose "selection replace + cursor positioning" as a primitive, add it — this same primitive will service any future wrap shortcut, but build it only because this ticket needs it, not on spec.
