# 0005 — FEA: Paragraph locks ("harden parts of the story")

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0002, #0004

## Problem
Even after caching and dismissal filtering, some paragraphs are "done" — published, hand-polished, intentionally unconventional — and should never be flagged again. Today the writer has to dismiss every flag individually, run after run, on text they don't want feedback on.

This is the literal feature the writer asked for as "hardening parts of the story."

## Scope
- `ParagraphMeta` gains `locked: bool` (default false), persisted in `chapter.json`
- Cache check in #0004 short-circuits to "no flags" for locked paragraphs — the model is never prompted for them, zero tokens spent
- Editor visual indicator for locked spans (faint background tint or gutter icon)
- Right-click on a paragraph in the editor → context menu with "Lock paragraph" / "Unlock paragraph"
- Inspector chapter tab shows count of locked paragraphs

## Out of scope
- Bulk lock/unlock UI
- Auto-locking based on heuristics (e.g. "lock when accepted N times")
- Locking sub-paragraph spans

## Acceptance criteria
- [ ] Locked paragraphs are never prompted to any pipeline (verify via prompt log)
- [ ] Lock state survives close/reopen
- [ ] Visual indicator in the editor for locked spans
- [ ] Right-click toggle works on the paragraph under the cursor
- [ ] Existing suggestions on a paragraph are preserved when it's locked — only new generation is suppressed
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- Right-click context menu is new for the editor surface; verify egui idiom or roll a small popup.
- "Paragraph under cursor" is computed by mapping cursor char index against `ParagraphMeta::char_range`.
- Locking a paragraph does NOT auto-mark its existing `proposed` suggestions as stale; the writer can still accept/dismiss them. New generation is the only thing gated.
