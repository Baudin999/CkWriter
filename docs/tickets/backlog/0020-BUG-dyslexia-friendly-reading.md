# 0020 — BUG: Dyslexia-friendly reading surface (chat font + editor)

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
The user is dyslexic. Two surfaces are currently uncomfortable to read:

1. **Chat panel font is too small.** `chat_bubble` (`src/ui/scope_panel.rs:1209-1224`) uses `ui.label(RichText::new(content))` with no explicit size, falling through to egui's default body size (~13–14 px on most setups). For a dyslexic reader, that's well below a comfortable threshold for sustained reading, especially for the long assistant replies the chat is designed to produce. The user notes this is true of "every AI agent" — it's a category-wide ergonomic gap, not just our chat.
2. **Editor reading surface is uncomfortable.** The manuscript editor (`src/ui/editor.rs`) uses egui's default `Proportional` family at the configured `font_size` and `line_height`. There is no dyslexia-friendly font option, no tunable letter/word/line spacing presets, and the default contrast/background may not be optimal for sustained reading.

This is the surface where the writer spends the most hours. Making it readable for him is not a polish item.

## Scope

### Chat panel
- Bump the chat-message body text to ~16–18 px (final value tunable by the user; pick a default and expose it).
- Apply to both committed messages (`scope_panel.rs:1168-1170`) and the streaming pending-assistant message (`scope_panel.rs:1171-1173`).
- The role label (`you` / `ai`) stays at its current `.small()` size — only the body changes.

### Editor reading surface
Add a small Settings → Reading section (extend `src/ui/settings_dialog.rs` and `src/settings.rs`) with persisted, user-tunable knobs:
- **Font family**: choose between the current Proportional default and a bundled dyslexia-oriented font (e.g. **OpenDyslexic** or **Atkinson Hyperlegible** — both have OFL licenses suitable for bundling). Bundle as a static asset under `assets/fonts/` and register with egui's font definitions at startup.
- **Font size**: numeric slider, default raised from current value to ~16 px, range 12–24 px.
- **Line height**: slider, default ~1.6× font size (currently lower), range 1.2–2.2.
- **Letter spacing** (`extra_letter_spacing` on `TextFormat`): slider, default ~0.4 (currently 0.1 per `editor.rs:291`), range 0.0–1.5.
- **Word spacing**: extra space after each space character. egui doesn't expose this directly; can be approximated via a layout pass in `build_layout_job` that emits a small zero-width spacer after each space, or deferred if it requires too much surgery — note in design.
- **Background tint**: choose between the current background and a softer cream/off-white tint (reduces glare for many dyslexic readers).

### Apply consistently
Whatever knobs end up in Settings should also drive the chat panel (it's a reading surface too), the inspector text labels, and any other long-form text view — not just the editor. The shared values live in one place (extend `src/theme.rs` or add a `ReadingSettings` next to `Settings`).

## Out of scope
- Dyslexia-friendly font for code areas (LaTeX commands, file paths) — those want a monospace and the dyslexia fonts are proportional.
- Text-to-speech / read-aloud — separate accessibility ticket if wanted.
- Per-paragraph "focus mode" highlighting (only the current paragraph at full opacity, others dimmed) — interesting but a separate feature ticket.
- Reflowing the entire UI chrome (panels, buttons) for accessibility — this ticket is the *reading surfaces* only.

## Acceptance criteria
- [ ] Chat message body text is rendered at the new larger default size (~16–18 px). Visible improvement over today.
- [ ] At least one dyslexia-oriented font (OpenDyslexic or Atkinson Hyperlegible) is bundled under `assets/fonts/`, registered with egui, and selectable in Settings.
- [ ] Settings → Reading has working sliders for font size, line height, and letter spacing; values persist across restarts via `src/settings.rs`.
- [ ] Switching font / size / spacing in Settings updates the editor and chat live (no restart needed).
- [ ] Background tint toggle works on the editor surface.
- [ ] Default values out-of-the-box are noticeably more readable than today (this is a judgment call confirmed by the user before close).
- [ ] All bundled fonts have OFL or compatible licenses; license file copied into `assets/fonts/<font>/LICENSE`.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Picking between OpenDyslexic and Atkinson Hyperlegible: OpenDyslexic is purpose-built for dyslexia but visually divisive; Atkinson Hyperlegible is broadly readable and many dyslexic readers prefer it. **Recommend bundling both** and letting the user choose — the cost is two ~200 KB font files, which is trivial.
- egui's font registration is per-`FontDefinitions`; add a new `FontFamily::Name("dyslexic-A")` entry alongside the proportional default and switch the editor's `FontId` family based on the setting.
- "Word spacing" is the trickiest knob; if it's too invasive in v1, ship without it and note in the ticket. The other four (family, size, line-height, letter-spacing) are the high-impact ones.
- The user has identified this as "every AI agent" makes the same mistake, so erring generous on default size (≥16 px, even at the cost of looking dense to non-dyslexic users) is the right call. The non-dyslexic user can shrink it; the dyslexic user has been silently bouncing off small text everywhere else.
- A future refinement worth flagging in the ticket but not this PR: the **Bionic Reading**-style highlighting (boldening the first half of each word) helps some dyslexic readers and can be implemented as a layout-pass in `build_layout_job`. Not committing to it here.
