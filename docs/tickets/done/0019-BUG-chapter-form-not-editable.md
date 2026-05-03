# 0019 — BUG: Chapter edit-form loses edits; build a unified forms framework

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
The Chapter tab form (`src/ui/scope_panel.rs:1226-1318`, `show_chapter`) writes into `app.chapter_draft` (a `ChapterDraft` clone of `ChapterMeta` defined in `src/app/mod.rs:52-62`). Edits commit only when the writer clicks "Save chapter info" (line 1308), and the draft can be silently destroyed by `seed_chapter_draft` (`src/app/book.rs:406-435`) — fired from `open_chapter` (`src/app/book.rs:130`) on chapter switch and from a defensive lazy-seed in `show_chapter` itself (`scope_panel.rs:1236`) any time `app.chapter_draft` is `None`. The writer reports edits "not sticking" — dirty draft + chapter switch (or any path that nulls the draft) silently reverts the typed text.

A second drift: read-only stats (`word_count`, `voice_score`, `last_coached_at`) read from `current_chapter.meta` (`scope_panel.rs:1238-1242, 1327, 1335`) while the editable fields read from the draft. The same screen shows two generations of state.

The chapter form is not the only form with this shape. Every form surface in the project hand-rolls its own draft / dirty / Save plumbing slightly differently. The duplicated glue *is* the underlying bug; patching just the chapter form would let the next form drift the same way. Inventory:

- **Chapter info** (`scope_panel.rs:1226-1318`) — `app.chapter_draft: Option<ChapterDraft>`, "Save chapter info" button, no Revert, no dirty indicator beyond the disabled-button state. Reseed-eats-edits bug.
- **Entity inspector** (`inspector.rs:23-135`) — `app.entity_dirty: Option<Entity>`, lift/mutate/write-back-each-frame, "Save" + "Revert" buttons.
- **Relation editor** inside the inspector (`inspector.rs:354`) and the **grid field helper** (`inspector.rs:440`) — same draft pattern, embedded.
- **Settings dialog** (`settings_dialog.rs`) already uses direct `&mut app.settings` + save-on-close. That's a different model (modal dialog, no draft) and stays as-is.
- Direct-binding non-form surfaces — chat input (`scope_panel.rs:1182`), search filter (`scope_panel.rs:265`), main editor (`editor.rs:170`), diff editor (`diff_view.rs:148`) — are not forms and stay direct.

## Scope
Build a small forms framework that every form surface above uses, then migrate them to it.

**Framework**
- A single `Form<T: Clone + PartialEq>` (struct preferred over trait — only adopt a trait if it gives meaningful sharing) lives in `src/ui/forms.rs`. It owns:
  - a `draft: T`,
  - a snapshot `original: T` for diff and revert,
  - a derived `dirty()` (`draft != original`),
  - a render entry point that takes the surface's body closure (returns `&mut T` for the widgets to bind to) and a commit closure (called on Save with `&T`).
- Save and Revert buttons rendered by the framework (not per surface). Save is enabled only when dirty; Revert is enabled only when dirty.
- Visible dirty indicator near the form title (small dot or `*`); each surface supplies the title.

**Discard-on-navigate**
- Whenever something would reseed an open form (chapter switch, entity selection change, book close, app close) the framework checks dirty first. If dirty, show a single shared modal: **"You have unsaved changes. Discard?"** with `Discard` / `Cancel`. Cancel aborts the navigation and the draft is kept; Discard drops the draft and the navigation proceeds.
- The pending navigation is staged on app state and resolved by the user's choice. Only one shared modal — not re-implemented per form.

**Migration**
- Chapter info → `Form<ChapterMeta>` (or a smaller `ChapterMetaEditable` view of the editable fields, if extracting the three fields keeps the snapshot cheap).
- Entity inspector → `Form<Entity>`. Existing commit closure (rename ripples, relation graph updates) stays — only the Save/Revert/dirty plumbing moves into the framework.
- Relation editor + grid field helper → consume the framework's draft state via the inspector's `Form<Entity>`.
- Read-only stats in the chapter form read from the form's draft `ChapterMeta` (or, if the form holds only the editable subset, from the live `current_chapter.meta` *but* refreshed alongside the form snapshot so the two halves never drift).

**Cleanup**
- `Cmd+S` no longer touches form drafts. Drop the `draft.dirty` shortcut handling at `src/app/mod.rs:345`. Save button is the only commit path for forms.
- Remove dead glue once it's fully replaced: `ChapterDraft` struct, `seed_chapter_draft`, `save_chapter_draft`, `app.chapter_draft`, `app.entity_dirty`, and the lift/mutate/write-back block at `inspector.rs:41-48`.

## Out of scope
- Settings dialog conversion (modal save-on-close model is fine).
- Optimistic UI for entity rename / restructure ripple effects — the framework only delivers a save event; the inspector's commit closure handles its ripples as today.
- Atomic write (tmp+rename) for `chapter.json` / entity JSON — separate ticket if filed; current `std::fs::write` (`src/book/chapter_meta.rs:75-83`) is unchanged here.
- A generic `FormField` widget abstraction over individual `TextEdit`s. The framework wraps the form, not each field.
- "Don't ask again this session" on the discard prompt — file later if it gets annoying.

## Acceptance criteria
- [x] `src/ui/forms.rs` contains the `Form<T>` framework; the chapter info form, entity inspector, relation editor, and grid field helper all consume it.
- [x] Chapter info form shows a visible dirty indicator near its title when any of summary / goals / plot_notes differs from the saved value.
- [x] Chapter info form Save button persists summary / goals / plot_notes and clears dirty; Revert discards the draft and clears dirty.
- [x] Entity inspector Save and Revert behave identically to today (commit closure preserves rename ripples and relation updates).
- [x] Switching chapters with a dirty chapter form blocks on a modal "You have unsaved changes. Discard?" prompt; Cancel keeps the user on the current chapter with draft intact; Discard drops the draft and switches.
- [x] Same prompt fires on entity selection change with the inspector dirty.
- [x] Same prompt fires on book close and app close with any form dirty.
- [x] After typing into a chapter field and triggering anything that previously reseeded `chapter_draft`, either the typed value is preserved or the user sees the discard prompt — never silent reversion.
- [x] Read-only stats (`word_count`, `voice_score`, `last_coached_at`) on the chapter form do not drift from the editable fields.
- [x] `Cmd+S` no longer references form drafts; the form Save button is the only commit path for forms.
- [x] Dead code removed: `ChapterDraft`, `seed_chapter_draft`, `save_chapter_draft`, `app.chapter_draft`, `app.entity_dirty`, the lift/mutate block at `inspector.rs:41-48` (or whatever fully replaces them).
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- `Form<T>` lifetime = while the surface is open. Construct with `Form::new(live: &T)` on first render, store on app state keyed by surface, drop on close. Each frame the surface re-asserts the live value via a method like `Form::rebase_if_clean(live: &T)` — a no-op if dirty (so the discard prompt is the only way to lose work) and a copy-from-live if clean (so external updates flow in).
- The shared discard-prompt modal lives at app level. Surfaces enqueue a "pending navigation" (a closure or enum variant); the modal resolves it. Keep the variant set tight — chapter switch, entity switch, book close, app close — rather than a generic boxed callback.
- Save closures take `&T` (the about-to-commit draft). The chapter form's closure does what `save_chapter_draft` / `update_chapter_meta` do today; the entity inspector's closure does what `commit_entity_edit` does today. The framework wires the buttons; the surfaces own the persistence.
- Read-only stats in the chapter form are easiest to keep coherent if the form's draft type is the full `ChapterMeta` (so stats and editable fields share one struct). If the snapshot cost is a problem, lift only the editable subset and refresh the read-only fields from `current_chapter.meta` in the same render frame the snapshot is taken — never independently.
- The framework lands the reseed-eats-edits fix by construction: the only path that mutates draft state is the framework's `rebase_if_clean` + `Form::revert`, both of which respect dirty.
- Keep the entity inspector's per-frame "lift / mutate / write back to draft" idiom — that's already the right shape for `Form<T>` where widgets take `&mut form.draft()`.
