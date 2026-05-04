# 0020 — BUG: Dyslexia-friendly reading surface (chat font + editor font choice + spacing)

**Type:** BUG
**Created:** 2026-05-03
**Refined:** 2026-05-04
**Depends on:** none

## Problem
The user is dyslexic. Two surfaces are uncomfortable to read for sustained sessions:

1. **Chat panel font is too small.** `chat_bubble` (`src/ui/scope_panel.rs:1470-1485`) uses `ui.label(RichText::new(content))` with no explicit size, falling through to egui's default body size (~13–14 px). For a dyslexic reader that's well below comfortable, especially for long assistant replies. The user notes this is true of "every AI agent" — a category-wide ergonomic gap.
2. **Editor lacks a font choice and a real letter-spacing knob.** The editor already uses iA Writer Quattro S at 18 px / 1.7× line-height (`src/theme.rs:82`, `src/settings.rs:57`, `src/ui/editor.rs:31`), which is a solid baseline. But there is no way to switch to a purpose-built dyslexia-friendly font, and `extra_letter_spacing` is hardcoded to 0.1 in `build_layout_job` (`src/ui/editor.rs:716`) with no slider.

This is the surface where the writer spends the most hours. Making it tunable for him is not a polish item.

## Scope

### Chat panel
- `chat_bubble` body text is rendered at the new shared `reading_font_size` (default 18 px) instead of egui's default. Both committed messages and the streaming pending-assistant bubble.
- The role label (`you` / `ai`) stays at its current `.small()` size — only the body changes.

### Reading settings (single source of truth)
Extend `src/settings.rs` so editor + chat both pull from the same knobs. Rename `editor_font_size` → `reading_font_size` (one shared value). Add:

- `reading_font: ReadingFont` enum — `IaWriterQuattro` (current default), `AtkinsonHyperlegible` (new default, see Design notes), `OpenDyslexic`.
- `reading_font_size: f32` — default 18.0, range 12.0–28.0 (renamed from `editor_font_size`).
- `reading_line_height_mult: f32` — default 1.7 (current const value), range 1.2–2.2. Replaces the `LINE_HEIGHT_MULTIPLIER` const in `src/ui/editor.rs:31`.
- `reading_letter_spacing: f32` — default 0.4 (up from hardcoded 0.1), range 0.0–1.5. Replaces the literal in `src/ui/editor.rs:716`.

Persist via the existing `Settings::save()` path. Add a serde `default = ` for each new field so old `settings.toml` files keep loading. Migrate any existing `editor_font_size` value during load (custom Deserialize or a `#[serde(alias = "editor_font_size")]`).

### Font registration
- Bundle two fonts under `assets/fonts/`:
  - `assets/fonts/atkinson-hyperlegible/AtkinsonHyperlegible-Regular.ttf` + `LICENSE` (OFL).
  - `assets/fonts/opendyslexic/OpenDyslexic-Regular.otf` + `LICENSE` (OFL).
- Register two new families in `src/theme.rs::install_fonts`:
  - `FontFamily::Name("reading-atkinson")` — Atkinson primary, then iA Writer Quattro fallback, then Ubuntu-Light, then fontawesome.
  - `FontFamily::Name("reading-opendyslexic")` — OpenDyslexic primary, then the same fallback chain.
- Keep the existing `WRITER_FAMILY` ("writer", iA Writer Quattro) as the iA Writer option.
- Add a helper `theme::reading_family(font: ReadingFont) -> FontFamily` that returns the right family for the setting. Editor (`src/ui/editor.rs:50 editor_family()`) and chat (`chat_bubble`) both call it.

### Settings dialog
Extend `src/ui/settings_dialog.rs` with a Reading section:
- font dropdown (3 entries)
- font size slider (suffix `" px"`)
- line height slider (suffix `"×"`, step 0.1)
- letter spacing slider (suffix `" px"`, step 0.1)

Live-applies (no restart). The existing dialog already auto-saves on close — same pattern; just add the new controls and bump `settings_dirty` on change.

### Apply consistently
Whatever knobs end up in Settings drive the editor and the chat. Inspector labels and short UI strings are out of scope (they're chrome, not reading prose).

## Out of scope
- Dyslexia-friendly font for code areas (LaTeX commands, file paths) — those want monospace and the dyslexia fonts are proportional.
- Background tint / cream "reading paper" surface — file as part of #NNNN (full theming system) where it can flip the whole palette coherently rather than clashing with surrounding dark chrome.
- Text-to-speech / read-aloud — separate accessibility ticket if wanted (CkTTSTT is the home for that).
- Per-paragraph "focus mode" highlighting — separate feature ticket.
- Word spacing slider — egui doesn't expose extra-space-after-space directly; would need a layout pass that emits zero-width spacers. Defer until the four real knobs land and the user has feedback.
- Bionic Reading-style first-half-bold — interesting, separate ticket.

## Acceptance criteria
- [x] Chat message body text renders at `reading_font_size` (default 18 px). Visibly larger than today's default-sized bubbles.
- [x] Atkinson Hyperlegible and OpenDyslexic are bundled under `assets/fonts/<font>/` with their LICENSE files; both are OFL-compatible.
- [x] Settings → Reading has working controls: font dropdown (3 entries), font size slider, line height slider, letter spacing slider.
- [x] All four controls update the editor and chat live (no restart).
- [x] Defaults: font = Atkinson Hyperlegible, size = 18 px, line-height = 1.7, letter-spacing = 0.4. (User confirms readability at close.)
- [x] Existing `settings.toml` files containing `editor_font_size = N` still load (alias or migration), and the value is honored as `reading_font_size`.
- [x] `LINE_HEIGHT_MULTIPLIER` const and the hardcoded `extra_letter_spacing: 0.1` in `src/ui/editor.rs` are gone — both flow from settings.
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Status notes
Implemented 2026-05-04. Editor + chat bubble now share a single `reading_*`
namespace in `Settings`; `reading_font` enum drives `theme::reading_family`,
which the editor (`editor_family`) and the chat transcript both call. The
`build_job` typography arguments were bundled into a `ReadingStyle` struct
to stay under clippy's `too_many_arguments` ceiling without an allow-list.
Old `editor_font_size = N` settings.toml entries continue to load via
`#[serde(alias = "editor_font_size")]` on `reading_font_size`. Coaching
cards, manuscript tree, inspector, and settings dialog all keep egui's
default proportional family — the reading knobs are reading-prose-only by
design (per Scope: "Inspector labels and short UI strings are out of scope
— they're chrome, not reading prose."). Visual confirmation of the new
defaults (Atkinson at 18 px / 1.7× / 0.4 px) deferred to user; quality
gates are clean.

## Design notes
- **Default font choice (Atkinson, not iA Writer):** the user is dyslexic and the whole point of this ticket is to bias defaults toward dyslexia-friendly. iA Writer Quattro is good but Atkinson is purpose-built for low-vision/dyslexic readers and is the safer out-of-the-box default. iA Writer remains available as one of the three options for direct comparison.
- **Three options in the dropdown** (per user request 2026-05-04): user wants to compare Atkinson, OpenDyslexic, and the current iA Writer side-by-side over real writing sessions before settling. Keeping all three is cheap (~400 KB total) and preserves the comparison.
- **Single `reading_*` namespace** (not `chat_*` + `editor_*`): the user confirmed one shared knob per dimension is the right model — chat is also a reading surface, and two-knob-per-dimension would just create drift.
- **Font registration in egui:** `FontFamily::Name("reading-atkinson")` etc. are added to `FontDefinitions::families` alongside the existing `WRITER_FAMILY`. The fontawesome fallback is appended to each so icon glyphs still resolve in any reading family.
- **Default letter spacing 0.4:** the current 0.1 is barely-there. 0.4 is a moderate bump that helps without looking gappy; the slider lets the user tune.
- **Default line-height 1.7:** preserves current behavior exactly. The slider exposes it for the user to push higher if needed.
- **Theming follow-up:** background tint was originally in this ticket; pulled out into a dedicated full-theming ticket because flipping just the editor background would clash with the surrounding dark panels. Done as #0030.

## Status notes
(Empty — refined and ready to start.)
