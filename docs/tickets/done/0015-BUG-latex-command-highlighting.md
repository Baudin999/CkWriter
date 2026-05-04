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
- [x] Typing `\nl` in the editor renders the four characters in pink immediately (no save/reopen needed).
- [x] Typing `\switch` renders all seven characters in pink.
- [x] Typing `\emph{hello}` renders `\emph{` and `}` in pink and `hello` in italic default-color text.
- [x] A nearly-correct typo like `\n1` or `\Switch` does **not** highlight (acts as the canary for "did I type it right").
- [x] LaTeX command highlighting layers correctly with entity hits (e.g. `\emph{Skari}` colors `\emph{` and `}` pink and shows `Skari` as an entity-colored italic span).
- [x] LaTeX command highlighting does not block revision underlines (revisions still render on top).
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Tokenizer can be a simple linear scan over `text` looking for `\nl\b`, `\switch\b`, and `\emph\{`. No regex dependency needed.
- "Word boundary" here means: end-of-string OR next char is not in `[A-Za-z0-9]` (LaTeX commands are a backslash followed by letters; `\nl1` would be a different command, not `\nl`).
- The shared pink color belongs in `src/theme.rs` next to the other entity colors so a future palette tweak hits all sites at once.
- Spans Vec is already sorted in `build_layout_job`; new LaTeX spans just get pushed in alongside the existing entity/revision spans. Italic for `\emph` content is set on the `TextFormat` via `font_id` family/style.

## Status notes
Parked 2026-05-03 after exploration only — no code written, working tree clean.

Key facts gathered (so tomorrow doesn't re-explore):
- The function the ticket calls `build_layout_job` is actually `build_job` at `src/ui/editor.rs:599`. Not at line ~285 — that area is the gutter/hover painter from #0023/#0024/#0025.
- Layer model lives at `src/ui/editor.rs:565-580`: `enum LayerKind { Entity, Revision }` and `struct Layer { start, end, color, kind, priority, selected }`. Atomic-boundary algorithm (lines 676-742) is what we extend.
- Current priorities: Entity = 0, unselected revision = 100..=103 (`100 + pipeline_byte`), selected = 255. For LaTeX-command-color-loses-to-entity, bump Entity to e.g. 50 and put `LatexCommand` at 0 (or any value < Entity, > 0 for clarity).
- Italic is **a `TextFormat` field**, not a font-family swap: `pub italics: bool` at `epaint-0.31.1/src/text/text_layout_types.rs:275`. So `\emph{...}` content italic is just `fmt.italics = true` — additive, doesn't fight entity color or revision underline. The "design notes" line in this ticket suggesting `font_id` family/style is wrong; use the bool.
- Theme colors live at `src/theme.rs`. Entity colors are at lines 12-13. Add `THEME_LATEX_COMMAND` (pink) there. There is no existing pink in the file — pick `Color32::from_rgb(0xf7, 0x6a, 0xc8)` or similar warm pink that reads on `EDITOR_PAGE` (#1c1c20). Confirm contrast before committing.
- Test pattern to follow: `run_build_job` helper at `src/ui/editor.rs:1450` and the three `#[test]`s after it (`overlapping_revisions_both_contribute_formatting`, etc.) — they assert on `job.sections[i].format.{color,underline.color,background,italics}`. Mirror that style for the new tests.
- Fingerprint at `layout_fingerprint` (line 915) hashes only text + hits + revisions + selected + font/wrap. **LaTeX commands are derived purely from `text`**, so the fingerprint already covers them — no fingerprint change needed.

Plan to execute tomorrow:
1. Add `LATEX_COMMAND` pink to `src/theme.rs`.
2. Add `LayerKind::LatexCommand` and renumber priorities so Entity > LatexCommand.
3. Write `fn latex_layers(text: &str) -> (Vec<Layer>, Vec<(usize,usize)>)` returning command-color spans + italic byte ranges; linear scan for `\nl`, `\switch`, `\emph{`, with word-boundary check (next char not in `[A-Za-z0-9]`) and same-paragraph brace match (stop at `\n` or end).
4. In `build_job`, push the LatexCommand layers into `layers` and apply italic by checking each sub-range's midpoint against the italic ranges, setting `fmt.italics = true` additively.
5. Tests: `\nl` pink; `\switch` pink; `\emph{hello}` braces pink + italic content; `\n1` and `\Switch` not highlighted; missing `}` → no span; entity inside `\emph{...}` keeps entity color + italic; revision underline survives over `\nl`.
6. `cargo clippy && cargo test` — must be 0/0.
