# 0017 — BUG: Editor UI flickers while typing (entity highlights)

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
While typing in the manuscript editor, highlighted entity names visibly flicker — the colored span and underline blink off and on each frame, especially on names that the writer is actively editing. The cause is in `src/ui/editor.rs::build_layout_job` (line ~285): the `LayoutJob` is reconstructed from scratch on every frame, including a fresh `extract::find` scan over the entire buffer. The scan and rebuild are both fast in the common case, but any per-frame variation (new `EntityHit` ranges as the writer types a new name, or temporary mismatches at character boundaries) causes the rendered `LayoutJob` to differ from one frame to the next, which egui presents as a flicker.

The writer experiences this as a distracting strobe on the names they're trying to write — exactly the worst place for visual noise.

## Scope
- Cache the `LayoutJob` by an input fingerprint and only rebuild when the fingerprint changes.
- Fingerprint inputs: `(text_hash, hits_hash, revisions_hash, selected_revision)`. `text_hash` can be a cheap `blake3` truncated to 8 bytes (we already use blake3 in #0002). `hits_hash` and `revisions_hash` likewise over their serialized form, or a manual hash over the salient fields.
- Store the cached `(fingerprint, LayoutJob)` either on `App` or in `egui::Memory` keyed by the editor `Id`. `Memory` is preferred so it survives panel toggles without bloating `App`.
- Investigate and fix the *root* cause of the per-frame variation — caching alone may mask but not eliminate it. Two known suspects:
  1. `extract::find` may produce slightly different hit ranges on partial input as the writer types past a name boundary; if so, ensure the result is deterministic for a given input and not order-dependent.
  2. `Stroke::new(1.0, color.linear_multiply(0.6))` returns a fresh struct every frame; if egui compares strokes by bit pattern this should be stable, but verify.

## Out of scope
- Reworking the entire editor render pipeline.
- Animating highlight transitions (i.e. fading instead of popping). The goal is "no flicker," not "smooth fade."
- Caching across chapter switches — clear the cache on chapter change.

## Acceptance criteria
- [ ] Typing inside or adjacent to a highlighted entity name produces no visible flicker on the highlight color or underline.
- [ ] Typing far from any highlight does not trigger a `LayoutJob` rebuild (verified via a counter or a `log::trace!` in dev builds, removed before merge).
- [ ] Adding or removing a character that *changes* the entity hits triggers exactly one rebuild for that frame.
- [ ] Switching chapters clears the cache (no stale highlights from the previous chapter bleed through).
- [ ] Toggling a revision's selected state still updates the underline thickness without delay.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- The fingerprint must include `selected_revision`, otherwise selecting a different revision won't re-render.
- A naive `text == cached_text` comparison is fine for now if buffers are short (chapters are well under 1 MB). blake3 is only worth it if the comparison shows up in a profile.
- If after caching the flicker persists, the issue is in egui's text-galley layout step itself, not in our `LayoutJob` construction; in that case the fix moves to investigating `request_repaint` cadence or galley caching.
