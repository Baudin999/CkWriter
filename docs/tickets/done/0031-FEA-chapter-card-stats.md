# 0031 — FEA: Chapter card stats — paragraphs and words

**Type:** FEATURE
**Created:** 2026-05-04
**Depends on:** none

## Problem
The Manuscript sidebar lists chapters by title only. To balance pacing across a book — some chapters running long, others a sketch — the writer wants paragraph and word counts visible at a glance, without opening each chapter. Both numbers already live on `chapter.meta` (`word_count: usize`, `paragraphs: Vec<ParagraphMeta>`); they are computed in `book::seed_meta` on chapter open and refreshed on save (`src/book/chapter_meta.rs:33`). The cost of *not* surfacing them is that pacing audits today require opening every chapter and eyeballing scroll height — which doesn't scale past a handful of chapters.

## Scope
- In `src/ui/file_tree.rs::draw_chapter_row`, append a small muted label to the right of the chapter title showing `¶ N · M w` (paragraph count, word count). Read both from `chapter.meta` — no new computation.
- Apply to manuscript rows AND orphan rows (orphan chapters still have meta on disk and are still part of the book the writer is balancing). The new affordance does not change row interaction; it sits before the per-row Open button on orphan rows, and at the right end of the row on manuscript rows.
- Format: `¶ {N}` and `{M:formatted} w` separated by ` · `, both in `theme::TEXT_MUTED`. Word counts ≥ 1000 use a thousands separator (so `3420` renders as `3,420`). Zero-paragraph and zero-word rows still render the badge — a fresh chapter showing `¶ 0 · 0 w` is information, not noise.
- The badge is right-aligned within the row (use `egui::Layout::right_to_left(Align::Center)` on a sub-region, the same pattern used elsewhere in the file).

## Out of scope
- A separate stats panel or summary footer over the whole list (a "total words across manuscript" counter could come later as its own ticket).
- Recomputing word counts in real time as the writer types in the editor — counts refresh on chapter save, which is the existing contract.
- Scene-level or beat-level breakdowns within a chapter.
- Stats on `All Files` view rows (those are raw filesystem entries, not chapters).
- Changes to `ChapterMeta` schema or the word-count algorithm.

## Acceptance criteria
- [x] Each manuscript row in the sidebar shows `¶ {paragraph_count} · {word_count} w` to the right of the title in muted color.
- [x] Each orphan row in the `ORPHANS` collapsing section shows the same badge, sitting before the existing Open pencil button.
- [x] Word counts ≥ 1000 use a thousands separator (`3420` → `3,420`).
- [x] Counts come from `chapter.meta.paragraphs.len()` and `chapter.meta.word_count`; no new computation introduced.
- [x] Editing and saving a chapter refreshes its row badge on the next `seed_meta` (existing behavior, verified visually).
- [x] `cargo clippy --all-targets --all-features` and `cargo test --all-targets --all-features` clean — 0 warnings, 0 errors.

## Design notes
- Word-count formatting: a small inline helper (e.g. `format_thousands(n: usize) -> String`) keeps things free of a heavy `num-format` dep. Insert commas every three digits from the right.
- Layout: keep the title's `selectable_label` taking the leftmost space, then an `ui.with_layout(right_to_left(Center))` sub-block for badge (and Open button on orphan rows). This matches the existing `MANUSCRIPT  +New` heading layout in `draw_manuscript`'s wrapper.
- Width pressure: a long chapter title in a narrow sidebar will visually crowd the badge. egui's right-to-left layout handles this by giving the badge its space first; the title `selectable_label` truncates or wraps according to its own settings. Acceptable for v1; if the title proves to clip too aggressively, file a follow-up to elide.
- The badge is not interactive (just `ui.label(...)`). Hover tooltip is unnecessary at v1 — the text is self-describing.
