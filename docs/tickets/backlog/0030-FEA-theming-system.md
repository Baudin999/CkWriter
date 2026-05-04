# 0030 — FEA: App-wide theming system (light/cream reading theme)

**Type:** FEATURE
**Created:** 2026-05-04
**Depends on:** none

## Problem
Today every color in the app is a `pub const` in `src/theme.rs` (`BG_PRIMARY`, `BG_PANEL`, `EDITOR_PAGE`, `TEXT_PRIMARY`, the gutter ramp, the revision colors, …). The app is hard-wired to one dark palette. Two consequences:

1. There is no way to switch to a light / cream / off-white reading surface, which many dyslexic readers prefer for sustained reading. (#0020 originally tried to bolt a "background tint" onto the editor only, but flipping just the editor page would clash with the surrounding dark chrome — the tint has to be a coordinated palette swap, not a one-off color.)
2. Any future palette work (high-contrast accessibility theme, seasonal experiments, user-customisable accents) has to touch every color call-site.

## Scope
- Replace the `pub const` palette in `src/theme.rs` with a `Theme` struct holding every color the app currently spells. Two built-ins: `dark` (current values, default) and `light` (a cream/off-white reading variant, paired with darker ink-style text and adjusted accent + gutter ramp so contrast still reads).
- Plumb the active `Theme` through wherever the consts are referenced (`src/ui/*`, `src/theme.rs::install`). Either store on `CkWriterApp` and pass into `ui::*` functions, or expose via `ctx.data()` so call sites do `theme::current(ctx).BG_PRIMARY`. Pick whichever causes less churn — likely a thin `theme::current(ctx) -> &Theme` accessor backed by `egui::Data`.
- Add `app_theme: AppTheme { Dark, Light }` to `Settings` and a Settings → Appearance dropdown to switch live (no restart). Default `Dark` to preserve current behavior for everyone except users who explicitly opt in.
- Light theme must keep the gutter ramp (`GUTTER_NEVER_PARSED`, `_CHANGED`, `_HAS_ISSUES`, `_CLEAN`, `_LOCKED`) distinguishable on a cream background — the current values are tuned for the dark page and will need re-picking.
- Light theme must keep entity highlight + revision underline colors readable against the lighter page (likely darken them).

## Out of scope
- User-customisable colors (color pickers, named profiles). Two built-in themes only for v1.
- Per-surface theming (e.g. dark editor inside light chrome). All-or-nothing palette swap.
- Syntax highlighting palette inside `\nl`/`\switch`/`\emph` markers — same color logic, just resolved against the active `Theme`.
- High-contrast accessibility theme as a separate variant — file as a follow-up once the framework lands.
- Migrating any baked-in `Color32::from_rgb(...)` values that aren't currently in `theme.rs` (audit during implementation; only the consts that exist today are in scope).

## Acceptance criteria
- [ ] `src/theme.rs` exposes a `Theme` struct (or equivalent) covering every color currently expressed as a `pub const`.
- [ ] Two built-in themes — `Theme::dark()` (matches current values exactly) and `Theme::light()` (cream/off-white reading variant with re-tuned gutter ramp + revision colors).
- [ ] Every existing call site of the old consts now resolves through the active theme. No hardcoded color values left in `src/ui/*` for things the theme owns.
- [ ] Settings → Appearance has an `App theme` dropdown (Dark / Light), persisted in `settings.toml`, live-applied (no restart).
- [ ] Both themes pass an eyeball test: gutter ramp readable, entity highlights visible, revision underlines visible, selected text legible against selection bg.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Spun out of #0020 (dyslexia-friendly reading) where a background-tint toggle was originally proposed but rejected because flipping only the editor page clashes with surrounding dark panels. The right shape is an app-wide theme swap, not a per-surface tint.
- Worth doing the `Theme` struct refactor once now rather than under time pressure later when a third or fourth variant gets requested.
- Keep the const names as `Theme` field names (`bg_primary`, `bg_panel`, `editor_page`, …) so call-site rename is mechanical.
- The `LATEX_COMMAND` pink, `REVISION_*` colors, and `GUTTER_*` ramp all need a second tuning for light mode — these are the trickiest because they have to be distinguishable both from each other and from the page.
