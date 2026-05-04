# 0028 — FEA: Super+I wraps or unwraps selection with `\emph{...}`

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** none

## Problem
Italic emphasis is the writer's most-used inline LaTeX wrap. Today it costs five characters and a cursor reposition for every emphasis: type `\emph{`, type the word, type `}`. Mid-sentence the friction is enough to skip the emphasis or break flow — and skipped emphasis weakens the prose.

## Scope
- **Super+I with a non-empty selection that is NOT already wrapped**: replace the selection with `\emph{<selection>}`. Cursor lands just after the closing `}`.
- **Super+I with no selection, cursor outside any `\emph{…}`**: insert `\emph{}` at the cursor and place the cursor between the braces, ready to type.
- **Super+I with the cursor inside an existing `\emph{…}` (no selection or selection fully inside the braces)**: unwrap — remove the surrounding `\emph{` and matching `}`, leaving the inner content. Cursor stays at the equivalent text position.
- **Super+I with a selection that exactly matches an `\emph{…}` span (including the braces)**: unwrap to the inner content.
- Brace matching is same-paragraph and non-recursive, mirroring the highlighter's behavior in #0015 — if the closing `}` is missing on the line, treat as "not wrapped" and fall back to wrap.
- Works in any paragraph; locked paragraphs (#0005) suppress coach generation, not typing, so wrapping is unaffected.
- Keybinding registered through whatever editor-shortcut plumbing the project already uses (match the existing pattern, don't invent a new one).

## Out of scope
- Bold (`\textbf`) or other wraps. Writer explicitly does not want them in v1.
- Configurable wrap commands. One keybinding, one command. Resist building a framework.
- Smart whitespace handling (e.g. moving leading space outside the wrap). v1 wraps exactly the selection.
- Unwrap of nested `\emph{\emph{…}}`: a single Super+I unwraps the innermost layer the cursor is inside; the outer remains. Press again to peel.

## Acceptance criteria
- [ ] Selecting `the rope` and pressing Super+I produces `\emph{the rope}` with the cursor positioned after the `}`.
- [ ] With no selection, pressing Super+I inserts `\emph{}` and places the cursor between the braces.
- [ ] Cursor inside `\emph{hello}` (between the braces, no selection) + Super+I produces `hello` with the cursor at the equivalent text offset.
- [ ] Selecting exactly `\emph{hello}` (braces included) + Super+I produces `hello` with the selection collapsed to the inner range.
- [ ] Selection that already starts with `\emph{` and ends with `}` does NOT double-wrap — it unwraps.
- [ ] Selection containing nested `\emph{…}` (e.g. selecting `\emph{a \emph{b} c}`) + Super+I peels exactly one layer.
- [ ] Multi-line selection wraps the whole span without losing line breaks.
- [ ] Existing editor shortcuts still work — Super+I doesn't shadow any current binding.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- User runs Arch with omarchy (Hyprland/Wayland), so Super is the natural modifier; do not use Cmd or Ctrl.
- Super+I verified unbound in `~/.config/hypr/` as of 2026-05-04, so the compositor will not swallow the keystroke. Re-verify if the project picks this up much later.
- After wrap or unwrap, fire whatever the editor's "text changed" path normally fires so the LaTeX highlighter (#0015) and downstream coach machinery see the new state.
- Detection for "is the cursor inside `\emph{…}`": scan the current paragraph for the nearest unmatched `\emph{` at or before the caret and the nearest matching `}` at or after; treat as wrapped if both exist on the same line. Reuse the same scanning shape as the #0015 tokenizer rather than introducing a second LaTeX parser.
- If the editor's input system doesn't already expose "selection replace + cursor positioning" as a primitive, add it — this same primitive will service any future wrap shortcut, but build it only because this ticket needs it, not on spec.
