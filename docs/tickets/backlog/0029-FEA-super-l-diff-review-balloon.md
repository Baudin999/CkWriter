# 0029 — FEA: Super+L paragraph diff review balloon

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** none

## Problem
After revising a paragraph, the writer wants a fast "did this edit help?" check — not a full coach run, not a panel of flags, just a verdict. Today the only way to ask is to run a pipeline against the whole chapter and read flags out of the panel; that's slow, expensive, and pulls the writer out of the prose. The writer asked for: keypress, send the diff to the LLM, get a balloon back.

## Scope
- **Super+L** in the editor with the cursor inside a paragraph: send the paragraph's before/after diff to the LLM; show the response in a small balloon anchored near the paragraph.
- **"Before"** = paragraph text as of the last save. **"After"** = current editor text for that paragraph. If the diff is empty (no edits since last save), show a one-line "no changes since last save" balloon and don't call the model.
- Prompt asks for a brief verdict (`better` / `worse` / `neutral`) plus one or two sentences of reasoning. Strict short-form output — this is a balloon, not a panel.
- Balloon dismisses on Esc, click outside, or the next Super+L. Does not pollute the coach suggestions panel; not persisted.
- Locked paragraphs (#0005) are still eligible — locks suppress coach pipelines, but explicit Super+L is the writer asking, not the coach offering.
- Surrounding context sent: the paragraph itself only, no chapter context. The diff is a local question; chapter-level voice is what the regular coach pipelines are for.

## Out of scope
- Persisting the verdict. If the writer wants to keep it, copy from the balloon. v1 is ephemeral.
- Multi-paragraph diff review.
- A keybinding to accept/reject the verdict — there's nothing to accept; it's information, not a suggestion.
- Folding the verdict into the existing coach suggestion model. This is a different surface.
- Threading the diff into next coach run's history. Separate ticket if needed.

## Acceptance criteria
- [ ] Cursor in an unmodified paragraph + Super+L → balloon "no changes since last save"; no LLM call issued.
- [ ] Cursor in a modified paragraph + Super+L → LLM call sends before+after; balloon shows verdict + 1–2 sentences.
- [ ] Balloon position is anchored to the paragraph in the editor (not the screen center).
- [ ] Esc dismisses the balloon; click outside dismisses; next Super+L replaces it.
- [ ] No new entries land in the coach suggestions panel.
- [ ] Locked paragraph + Super+L still works (lock does not gate this path).
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Baseline = last-saved paragraph text. Reading from the saved chapter snapshot is cheaper than maintaining a separate "session baseline" structure. After a Save the diff resets to empty for every paragraph automatically — that's the right semantics.
- User runs Arch + omarchy (Hyprland), so Super is the natural modifier; do not use Cmd or Ctrl.
- Prompt skeleton (subject to iteration once we see model output): "A writer just revised this paragraph. BEFORE: <before>. AFTER: <after>. Has this change improved the prose? Reply with one of {better, worse, neutral} on the first line, then one or two sentences of reasoning. Do not propose new edits."
- Balloon UI in egui: an `egui::Area` or borderless `egui::Window` anchored near the paragraph rect, fixed width (~400px), dismissable on Esc or click-out. Reuse whatever popup pattern the editor already has if one exists.
- This is the "AI sees the change" feedback loop the writer described. The diff goes in, the verdict comes back. We do NOT (yet) feed accepted/rejected suggestions back into next-run history — that's a separate ticket if needed.
