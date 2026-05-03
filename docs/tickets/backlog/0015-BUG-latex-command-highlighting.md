# 0015 — BUG: `\nl`, `\switch`, `\emph` not highlighted in editor

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
The editor's syntax pass in `src/ui/editor.rs` (`build_layout_job`, ~line 285) only colors entity hits and revision anchors. It has no LaTeX command tokenizer, so the project's three most-typed custom commands render as plain prose:

- `\nl` — paragraph-break-within-scene marker.
- `\switch` — POV switch marker.
- `\emph{...}` — italic emphasis.

The writer is using these constantly while drafting and cannot tell at a glance whether the slash actually landed (typos like `\n1` or `\Switch` look identical to a correct command). For `\emph` the issue is doubly bad: the content inside the braces should *render* italic in the editor too, since that's what it will look like in the PDF — without it, the emphasis is invisible until the next compile.

## Scope
- Extend the editor's layout pass with a small LaTeX-command tokenizer that produces spans for `\nl`, `\switch`, and `\emph{…}`.
- `\nl` and `\switch`: color the command (including the leading backslash) in pink (`Color32::from_rgb(255, 105, 180)` or whichever pink already lives in `src/theme.rs` if there is one — add `THEME_LATEX_COMMAND` if not).
- `\emph{…}`: the `\emph{` and the closing `}` render in the same pink; the content between the braces renders in the default text color but italic.
- Tokenizer must compose with existing entity-hit and revision spans (don't clobber them; layered formatting via existing `spans` Vec is fine — pick the priority order: revision underline > entity color > LaTeX command color, but italic for `\emph` content is independent and additive).
- Brace matching for `\emph` is non-recursive (no nested `\emph`); if the closing `}` is missing on the same paragraph, leave the span un-applied rather than coloring to end-of-text.

## Out of scope
- A general LaTeX syntax highlighter (sections, environments, math). This ticket is the three custom commands the writer actually uses while drafting.
- Auto-completion of `\nl` / `\switch` / `\emph`.
- Highlighting these in the chat panel or PDF preview.

## Acceptance criteria
- [ ] Typing `\nl` in the editor renders the four characters in pink immediately (no save/reopen needed).
- [ ] Typing `\switch` renders all seven characters in pink.
- [ ] Typing `\emph{hello}` renders `\emph{` and `}` in pink and `hello` in italic default-color text.
- [ ] A nearly-correct typo like `\n1` or `\Switch` does **not** highlight (acts as the canary for "did I type it right").
- [ ] LaTeX command highlighting layers correctly with entity hits (e.g. `\emph{Skari}` colors `\emph{` and `}` pink and shows `Skari` as an entity-colored italic span).
- [ ] LaTeX command highlighting does not block revision underlines (revisions still render on top).
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Tokenizer can be a simple linear scan over `text` looking for `\nl\b`, `\switch\b`, and `\emph\{`. No regex dependency needed.
- "Word boundary" here means: end-of-string OR next char is not in `[A-Za-z0-9]` (LaTeX commands are a backslash followed by letters; `\nl1` would be a different command, not `\nl`).
- The shared pink color belongs in `src/theme.rs` next to the other entity colors so a future palette tweak hits all sites at once.
- Spans Vec is already sorted in `build_layout_job`; new LaTeX spans just get pushed in alongside the existing entity/revision spans. Italic for `\emph` content is set on the `TextFormat` via `font_id` family/style.
