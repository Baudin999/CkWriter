# 0020 — BUG: Dyslexia-friendly reading surface (one global font + three sizes + editor knobs)

**Type:** BUG
**Created:** 2026-05-03
**Refined:** 2026-05-04
**Re-refined:** 2026-05-04 (mid-flight pivot — see `## Pivot` below)
**Depends on:** none

## Problem
The user is dyslexic. The whole app is a reading surface for him — not just
the editor and the chat panel. A first pass that only retuned those two
landed (commit `9751dbf`) but left the rest of the UI (coaching cards,
chapter form prose, paragraph notes, inspector entity prose, dismissal
reason, chat input box, every label/button in chrome) at egui's default
proportional family at default sizes — too small and not dyslexia-friendly.

Sub-problems:
1. **No global font.** egui's default proportional family is used for every
   surface that doesn't explicitly opt in. There's no single switch to
   route the dyslexia-friendly font everywhere.
2. **No size hierarchy.** Today every label picks its own size ad-hoc
   (`.small()` here, no size there, `RichText::size(18.0)` somewhere
   else). Without a small set of named sizes the app drifts.
3. **Editor lacks a column-width control.** `MAX_COLUMN_WIDTH` /
   `MIN_COLUMN_WIDTH` are hardcoded constants in `src/ui/editor.rs:23-24`.
   The user wants to widen/narrow the column to match comfortable line
   lengths.
4. **Editor letter-spacing + line-height knobs were applied globally** in
   the first pass — but those are editor-specific tuning knobs (sustained
   prose reading), not appropriate for chat bubbles, lists, buttons, or
   chrome. They need to be scoped to the editor.

## Pivot
The first pass implemented `reading_*` as a "shared editor + chat"
namespace and left the rest of the app on egui defaults. After the user
inspected the result (2026-05-04) the redesign is:

- **One font, app-wide.** The chosen font (Atkinson default) drives every
  proportional text style in egui — Body, Heading, Button, Small. No
  per-surface opt-in: install it once into `egui::Style::text_styles`
  and the whole app inherits.
- **Three named sizes.** `font_size_normal` (default 18 px),
  `font_size_header` (default 22 px), `font_size_info` (default 13 px).
  Map: Body/Button → normal, Heading → header, Small → info. Three sizes
  cover everything; anything that wants more nuance is wrong.
- **Editor-only knobs.** Line-height, letter-spacing, and the new
  column-width slider apply only to the editor's `build_job` /
  layout. They do not touch the global style. Chat bubbles, coaching
  cards, chapter form fields, paragraph notes, inspector prose — all
  inherit the global font + `font_size_normal` for body and `..._info`
  for muted/`small` text.

## Scope

### Settings (replace the current `reading_*` namespace)
Rename / add fields in `src/settings.rs`:

- `reading_font: ReadingFont` — already exists. Keep. This is the
  global font choice. Variants: `AtkinsonHyperlegible` (default),
  `OpenDyslexic`, `IaWriterQuattro`.
- `font_size_normal: f32` — default 18.0, range 12.0–28.0. Renamed from
  `reading_font_size` (which was renamed from `editor_font_size`). Keep
  the serde aliases for both old names so old `settings.toml` files load.
- `font_size_header: f32` — default 22.0, range 14.0–34.0. **New.**
- `font_size_info: f32` — default 13.0, range 10.0–18.0. **New.**
- `editor_line_height_mult: f32` — default 1.7, range 1.2–2.2. Renamed
  from `reading_line_height_mult` (alias the old name).
- `editor_letter_spacing: f32` — default 0.4, range 0.0–1.5. Renamed
  from `reading_letter_spacing` (alias the old name).
- `editor_column_width: f32` — default 760.0, range 480.0–1000.0. **New.**
  Replaces the `MAX_COLUMN_WIDTH` const in `src/ui/editor.rs`.

### Global style installation
In `src/theme.rs::install` (or a new helper), after `install_fonts`,
populate `egui::Style::text_styles` from settings:

- `TextStyle::Body` → `FontId::new(font_size_normal, reading_family(font))`
- `TextStyle::Heading` → `FontId::new(font_size_header, reading_family(font))`
- `TextStyle::Button` → `FontId::new(font_size_normal, reading_family(font))`
- `TextStyle::Small` → `FontId::new(font_size_info, reading_family(font))`
- `TextStyle::Monospace` → unchanged (keeps egui's default monospace
  family at `font_size_normal` for legibility). Atkinson and
  OpenDyslexic are proportional; forcing them onto code/keyboard-hint
  text reads wrong.

This must be re-applied each frame (or each settings change) so the
font dropdown and the three size sliders live-apply.

### Editor (column width, line height, letter spacing)
- Replace `MAX_COLUMN_WIDTH` const with `app.settings.editor_column_width`.
  `MIN_COLUMN_WIDTH` (the responsive lower bound for narrow windows) can
  stay; the new slider drives the upper bound.
- Editor `build_job` already pulls `editor_line_height_mult` and
  `editor_letter_spacing` (was `reading_*`) — just follow the rename.
- Editor explicitly uses `font_size_normal` for body and the chosen
  `reading_family` for the prose buffer. (Same as today, the rename is
  purely the field name.)

### Revert per-surface explicit font on chat bubble
The first pass added `RichText::new(content).font(FontId::new(...))`
inside `chat_bubble` (`src/ui/scope_panel.rs:1470-1494`). With the global
style in place, plain `ui.label(RichText::new(content))` already inherits
the reading font at `font_size_normal`. Drop the explicit `.font(...)` —
single source of truth.

### Settings dialog
Replace the current Reading section in `src/ui/settings_dialog.rs` with:

- font dropdown (3 entries) — global
- font size: normal (suffix `" px"`)
- font size: header (suffix `" px"`)
- font size: info (suffix `" px"`)
- editor column width (suffix `" px"`)
- editor line height (suffix `"×"`, step 0.1)
- editor letter spacing (suffix `" px"`, step 0.1)

Section headers within the dialog: "Reading (app-wide)" and "Editor".

## Out of scope
- Monospace family swap. Egui's default monospace stays. Code identifiers,
  LaTeX commands, keyboard hints (`⌘↵`) all keep egui's monospace family.
  (User confirmed 2026-05-04: keep monospace as monospace.)
- Background tint / cream "reading paper" surface — `#0030` (full theming).
- Text-to-speech / read-aloud — CkTTSTT.
- Per-paragraph "focus mode" highlighting — separate feature.
- Word-spacing slider — egui doesn't expose it directly; defer.
- Bionic Reading-style first-half-bold — separate ticket.

## Acceptance criteria
- [ ] Every proportional text surface in the app (chat input + bubbles,
      coaching cards, chapter form fields, paragraph notes, inspector
      entity prose, settings dialog labels, list rows) renders in the
      chosen `reading_font` at `font_size_normal` for body, `font_size_info`
      for muted/`.small()`, `font_size_header` for any heading widget.
- [ ] Monospace text (keyboard hints, any code-style spans) stays in
      egui's default monospace family.
- [ ] Editor uses the same `reading_font` + `font_size_normal`, plus its
      own `editor_line_height_mult` and `editor_letter_spacing`.
- [ ] Editor column max width tracks the new `editor_column_width`
      slider; the column responds to changes the same frame.
- [ ] Settings dialog has the seven controls listed under
      `### Settings dialog`, organized in two sections (app-wide vs.
      editor). All live-apply.
- [ ] Defaults: font = Atkinson Hyperlegible, normal = 18 px,
      header = 22 px, info = 13 px, line-height = 1.7×,
      letter-spacing = 0.4 px, column-width = 760 px.
- [ ] Atkinson and OpenDyslexic remain bundled under `assets/fonts/`.
- [ ] Old `settings.toml` keys keep loading via aliases:
      `editor_font_size` → `font_size_normal`,
      `reading_font_size` → `font_size_normal`,
      `reading_line_height_mult` → `editor_line_height_mult`,
      `reading_letter_spacing` → `editor_letter_spacing`.
- [ ] `MAX_COLUMN_WIDTH` const is gone (replaced by setting). `MIN_COLUMN_WIDTH`
      may remain as the responsive lower bound.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` both
      return zero errors and zero warnings.

## Status notes
First pass (commit `9751dbf` on master) shipped:
- Bundled Atkinson Hyperlegible + OpenDyslexic under `assets/fonts/`
  with OFL `LICENSE` files. **Keep.**
- Registered `FontFamily::Name("reading-atkinson")` and
  `FontFamily::Name("reading-opendyslexic")` in `theme.rs::install_fonts`,
  with `theme::reading_family(ReadingFont) -> FontFamily`. **Keep.**
- Settings field `reading_font: ReadingFont`. **Keep.**
- Settings field `reading_font_size`, `reading_line_height_mult`,
  `reading_letter_spacing`. **Rename** per `## Scope`, keep aliases for
  the old names (and the original `editor_font_size`).
- Editor: `LINE_HEIGHT_MULTIPLIER` const removed; `extra_letter_spacing`
  literal removed; `ReadingStyle` struct bundles typography args. **Keep.**
- `chat_bubble` accepts an explicit `body_family` + `body_size`. **Revert** —
  the global style installation will provide both.
- Settings dialog has font dropdown + 3 sliders. **Replace** with the
  seven controls in this ticket.

## Design notes
- **Why "same font everywhere" not "reading surfaces only":** the first
  pass split the app into "reading prose" (editor + chat) and "chrome"
  (lists, labels, dialogs). The user is dyslexic — chrome is also text
  he has to read. The cleaner mental model is "the app has one font;
  the editor has extra spacing on top." (User feedback, 2026-05-04.)
- **Why three sizes (normal / header / info):** maps cleanly onto egui's
  built-in `TextStyle` enum (Body, Heading, Small). More sizes invites
  drift; fewer can't distinguish a section header from body. Three is
  the right number. (User pitch, 2026-05-04.)
- **Why editor-only line-height + letter-spacing:** these are
  per-character `TextFormat` knobs that egui applies through `LayoutJob`,
  not through `Style::text_styles`. They only kick in inside the
  editor's `build_job` anyway. Scoping them in settings/UI as
  "Editor" rather than "Reading" matches reality and avoids the
  implication that they affect chat / cards.
- **Why an editor column-width slider:** today it's a hardcoded
  `MAX_COLUMN_WIDTH = 760.0`. Comfortable line length for a dyslexic
  reader is personal — needs to be tunable like font size.
- **Monospace stays monospace** (user confirmed 2026-05-04): Atkinson
  and OpenDyslexic are proportional. Forcing them onto `⌘↵` and any
  code-style hints would look wrong. Egui's default monospace family
  is left untouched.
- **Live-apply via per-frame `set_style`:** egui supports calling
  `ctx.set_style(...)` each frame; cost is trivial and it sidesteps
  having to track "did settings change since last frame." Same pattern
  the existing dialog already uses for save-on-close.
