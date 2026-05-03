# 0026 — BUG: Clicked coach card has no visible indicator and overlapping spans disappear

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
Two related visibility failures around the coach revision panel and the editor highlights it drives. Both make it hard to tell which card you just clicked.

**1. Selected card has effectively no editor-side indicator.**
Clicking a coach card jumps the editor to the anchor (via `select_revision` → `jump_to_anchor`) but the writer can't see *which* span got selected. In `src/ui/editor.rs:548-553` the selected branch paints:

```rust
f.underline = Stroke::new(3.0, color);
f.background = theme::REVISION_SELECTED_BG;
```

with `REVISION_SELECTED_BG = #332c2c` (`src/theme.rs:20`). The editor page is `EDITOR_PAGE = #1c1c20`. The luminance delta is ~7% — visually a non-event on a dark background, especially when the cursor is also blinking on top. The 3px-vs-2px underline difference is similarly marginal, and is invisible the moment the span is short.

**2. Overlapping revision spans are silently dropped, so the second card looks dead.**
`build_job` in `src/ui/editor.rs:559-569` walks spans sorted by start and skips any span whose start is before the running `cursor`:

```rust
for (s, e, fmt) in spans {
    if s < cursor || e > text.len() || s >= e {
        continue;
    }
    ...
    cursor = e;
}
```

Two coach flags whose quotes overlap (very common when prose, show-don't-tell, and spelling all flag the same sentence) collide on this check: only the first wins, the rest paint nothing. Clicking the dropped card jumps the cursor (so the writer knows *something* happened) but the underline / background never appears for it, which reads as "the click did nothing." Selecting a dropped card also can't be visually distinguished from selecting any other dropped card.

The cost compounds with #0023's per-paragraph play button — running show + prose + spelling on one paragraph is exactly the workflow that produces overlapping spans.

## Scope
- Selected indicator: replace the dim grey background with a tinted version of the revision's own colour so the highlight matches the card chip the writer just clicked. Bump alpha enough to read on `EDITOR_PAGE` without obscuring text. Keep (or thicken) the wider underline so short spans still register.
- Overlapping spans: stop dropping spans whose range starts before `cursor`. Build a layered span model so every revision contributes its underline. Two acceptable shapes; pick one in design notes:
  1. **Sub-span split.** Slice overlapping ranges into atomic sub-ranges and merge their formats (combine underlines, pick the strongest background). egui's `TextFormat` only carries one underline `Stroke`, so layered underlines collapse to a single colour — for overlap regions, paint the *selected* span's colour if any participant is selected, otherwise the highest-priority pipeline.
  2. **Stacked galley overlay.** Draw the base text with the lowest layer's spans, then paint additional underlines as separate `Shape::line_segment`s under the galley for each overlapping revision, offset vertically (e.g. `-1px`, `-3px`) so two underlines stay distinguishable.
- Either way, the selected revision must remain unambiguously identifiable inside an overlap region (one tell-tale: its tinted background or a heavier underline only it owns).
- Add a unit test in `src/ui/editor.rs` (alongside the existing `selected_revision_perturbation_changes_fingerprint` at line 928) asserting that a `LayoutJob` built from two overlapping revisions contains formatting attributable to *both* — e.g. count of formatted span runs covering the overlap region is ≥ 2, or the second revision's colour appears somewhere in the job's section list.

## Out of scope
- Reworking how cards are laid out in the side panel. The cards themselves don't overlap visually; the "overlap" the writer experiences is in the editor highlight layer. (If panel-card visual separation turns out to be a real complaint after this lands, file a follow-up.)
- Changing the anchor algorithm or the prose-fallback handling. #0021 owns that.
- Animation / pulse effects on the selected highlight. A static, high-contrast indicator is the goal — animation can be a follow-up if static still isn't enough.
- Making `egui::TextFormat` carry multiple underlines upstream. Work around the single-underline limitation locally.

## Acceptance criteria
- [x] Selecting a coach card produces an editor highlight that is unambiguously visible at a glance on `EDITOR_PAGE` — verified by manual test: open a chapter with at least one coach flag, click the card, confirm you can locate the highlighted span without scrolling around. _(writer-confirmed 2026-05-03)_
- [x] When two coach flags anchor to overlapping ranges in the same paragraph (set this up by running show + prose + spelling and finding a sentence flagged by ≥2 pipelines), both spans have visible editor-side affordance: each can be selected independently and the active selection is visually distinct from the inactive overlap participant. _(writer-confirmed 2026-05-03)_
- [x] The previously-dropped span is no longer silently skipped — verified by a unit test against `build_job` (or whatever the refactored span builder becomes) that proves both revisions contribute formatting in an overlap. _(`overlapping_revisions_both_contribute_formatting` + `coincident_unselected_overlap_keeps_secondary_color_visible` in `src/ui/editor.rs`)_
- [x] `cargo clippy --all-targets -- -D warnings` and `cargo test` both return zero warnings and zero errors. _(166 passed, 0 warnings)_

## Status notes
Implemented 2026-05-03. Code-side acceptance is met; visual checks (1) and (2) need the writer's eye before this can move to `done/`.

What changed:
- `src/ui/editor.rs::build_job` rewritten as a layered sub-span splitter. Every revision/entity becomes a `Layer { start, end, color, kind, priority, selected }`; the function dedups all layer boundaries into atomic sub-ranges and merges the participants per sub-range. The old `if s < cursor { continue; }` skip is gone — overlapping spans no longer drop the second flag.
- Per-sub-range merge rules: primary (highest-priority participant) owns the underline color/width and the foreground color. Selected pins priority to 255 (always loudest); revision priority is `100 + pipeline_byte` so spelling-family > prose > show > voice in unselected overlaps. Entities sit at priority 0.
- Selected indicator: `f.background = revision_tint(selected.color, SELECTED_TINT_ALPHA)` (`0x55` ≈ 33%) — the chip color the writer clicked, instead of the old flat `#332c2c`.
- Unselected overlap (≥2 revisions, no selection in this sub-range): `f.background = revision_tint(secondary.color, OVERLAP_TINT_ALPHA)` (`0x28` ≈ 16%) — the lower-priority flag's color shows through as a quiet hint, so a click on the dropped card no longer reads as a no-op even when ranges fully coincide.
- `theme::REVISION_SELECTED_BG` is no longer used by the editor; left in the theme module because `src/ui/scope_panel.rs::revision_card` still uses it for the side-panel chip background (out of scope per the ticket).

Tuning knobs if visual check finds the tints off:
- `SELECTED_TINT_ALPHA` / `OVERLAP_TINT_ALPHA` constants at the top of `build_job` in `src/ui/editor.rs`. Bump alpha for more saturation, reduce for less. The two are independent — the selected indicator is the load-bearing one for criterion (1).
- If the chip-tinted selected background reads as too saturated on a specific pipeline color, the fallback path in the design notes (a brightened fixed off-tone) is a small follow-up edit.

Manual test recipe for the writer:
1. Open a chapter with at least one coach flag of any pipeline.
2. Click the card. Highlight should match the chip color and be obvious without hunting.
3. Run show + prose + spelling on a paragraph until ≥2 pipelines flag the same sentence; click each card in turn. Each click should land a visibly different highlight on the overlapping range.

## Design notes
Recommended approach: **sub-span split** (option 1). It keeps the rendering inside `LayoutJob` and avoids a second draw pass that has to track scroll, wrap, and font-metrics changes from the editor. The single-underline-per-format limit is genuine but workable: in the overlap region we pick the underline colour by priority (selected > spelling-family > pipeline), and we use the *background* tint to communicate the second flag's existence — e.g. selected flag wins the underline colour, the unselected overlap participant's colour shows through as the background tint. This is a deliberate compression: "two flags here, the selected one is X" is the message, not "render every flag's colour pixel-perfectly."

For the selected-indicator contrast fix specifically, a starting point worth trying: build the background as `revision_color(rev).gamma_multiply(0.35).additive()` (or compose with `Color32::from_rgba_unmultiplied(r, g, b, 0x55)`) so the tint inherits the card's chip colour. If that proves too saturated for prose, retreat to a fixed off-tone (e.g. a brightened version of `REVISION_SELECTED_BG` around `#4a3a3a`) — but tie it to the chip colour first, since the whole point is "the card I clicked matches the highlight I'm looking at."

Open question for refinement: the user's original report said *"when multiple cards overlap it's hard to separate them"* — which I'm reading as the editor-side underline overlap described above. If during refinement we discover they actually meant the side-panel cards visually running together (they don't currently overlap geometrically, but adjacent `Frame::group` borders can read as one block), expand the scope here or split into a second ticket.
