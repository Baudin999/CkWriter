# 0014 — BUG: Manuscript rows: click and drag collide

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
In the left-panel manuscript list, every row is wrapped in `dnd_drag_source` (see `src/ui/file_tree.rs:148` and the comment block at lines 137–141, 254–264). egui's hit-test suppresses click-only widgets sitting under a drag-only widget, so the row's `selectable_label` cannot receive clicks at all — the workaround today is a separate "Open" button that sits on top of the drag layer.

The writer's mental model is: a quick click selects/opens a chapter, a deliberate hold-and-drag reorders it. Today they have to aim for a small icon button instead. The collision also bleeds into other row affordances we'll add later (e.g. inline rename).

## Scope
- Replace the unconditional `dnd_drag_source` wrapper with a "press-and-hold" gate: dragging only begins after the pointer has been held on the row for ~500 ms (or has moved more than the drag-start threshold, whichever comes first).
- Until that threshold elapses, the row behaves as a normal `selectable_label` — pointer-up within the threshold counts as a click and opens the chapter.
- Once the threshold elapses (or the pointer moves past the drag threshold), engage the existing `dnd_drag_source` flow unchanged.
- Remove the now-redundant separate "Open" button on the row, since clicking the row itself opens the chapter.
- The reorder/delete/edit affordances on the right of the row stay as they are.

## Out of scope
- Touchscreen long-press behavior tuning.
- Keyboard reorder (a separate accessibility ticket if wanted).
- Visual hover/press feedback redesign for the row.
- Same treatment for non-manuscript rows (orphans, info files) — they already work correctly because they are not drag sources.

## Acceptance criteria
- [ ] A pointer-down + pointer-up within 500 ms on a manuscript row opens that chapter (no need to aim for the "Open" button).
- [ ] A pointer-down held for ≥500 ms (or moved ≥ egui's drag threshold) on a manuscript row begins a drag, with the same drop targets and reorder behavior as today.
- [ ] Dragging onto another manuscript row drops before that row; dragging past the last row drops at the end (existing behavior preserved).
- [ ] Cancelling the drag (releasing outside any drop zone) does not also fire a click — the chapter does not open spuriously after a cancelled drag.
- [ ] The separate "Open" button on the manuscript row is removed; no UI regression on rows that are currently selected.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- 500 ms is the proposed default. Tunable to 400/600 ms if it feels off in practice; record the final value in a constant in `src/ui/file_tree.rs` with a one-line rationale.
- egui has no built-in "long-press → drag" widget; expect to track a per-row `Id`-keyed timestamp in `egui::Memory` (pointer-down time) and only enter `dnd_drag_source` once `now - press_time >= 500ms` OR pointer movement exceeds `ctx.input(|i| i.pointer.is_decidedly_dragging())`.
- The trailing drop zone after the last row (`src/ui/file_tree.rs:166`) does not need changes — it has no click semantics to collide with.
