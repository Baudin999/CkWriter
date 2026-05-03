# 0019 — BUG: Chapter edit-form is non-editable; forms must be 2-way bound

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
The Chapter tab form (`src/ui/scope_panel.rs:1226-1318`, `show_chapter`) writes into `app.chapter_draft` (a separate `ChapterDraft` struct cloned from the loaded `ChapterMeta`), and only commits back when the writer clicks "Save chapter info" (line 1308). Symptoms the writer reports:

- Edits to summary / goals / plot_notes appear to "not stick" — typing into the field looks fine, but the value reverts the moment something else triggers a chapter reseed (chapter switch, save, a coach run, etc.).
- The form's read-only stats (`word_count`, `voice_score`) are loaded from `current_chapter.meta`, while the editable fields are loaded from `chapter_draft` — these can drift, so the same screen shows two different generations of state.

The user's framing: **forms in this app should be 2-way bindable**. A `TextEdit` bound to `&mut field` should write straight back into the canonical state, not into a side-buffer that needs an explicit "Save" click to flush.

This recurs for the same reason in other forms (entity inspector, etc.), so the fix should land a pattern, not just patch one tab.

## Scope
- Replace the `chapter_draft` indirection in the Chapter tab with direct binding into `current_chapter.meta`. `TextEdit` writes go straight to the loaded `ChapterMeta`.
- Persist on change: when `response.changed()` fires, mark the chapter dirty and let the existing autosave / on-blur / on-chapter-switch path write `chapter.json`. No explicit "Save chapter info" button.
- Audit other forms in the project (entity inspector at `src/ui/inspector.rs`, settings dialog at `src/ui/settings_dialog.rs`) for the same indirection pattern. If any of them route edits through a draft/clone instead of a direct `&mut`, list them in this ticket and convert them in the same change.
- Establish a one-paragraph convention (in `CLAUDE.md` or a comment block at the top of `scope_panel.rs`/`inspector.rs`) describing the expected pattern: forms bind to the live model with `&mut`, persistence is triggered by `changed()` plus a debounce or on-blur, never by a manual Save button.

## Out of scope
- Implementing a generic `Form<T>` widget. The fix is the binding pattern, not a new abstraction.
- Optimistic UI for entity rename / restructure flows (those have correctness reasons to stage changes).
- Conflict handling for "the file changed on disk while you had it open" — separate concern.

## Acceptance criteria
- [ ] Typing into summary / goals / plot_notes in the Chapter tab updates `current_chapter.meta` directly on each keystroke.
- [ ] Switching chapters or closing the book persists the latest in-memory `ChapterMeta` to `chapter.json` without requiring a "Save" click.
- [ ] The "Save chapter info" button is removed (or downgraded to a no-op safety net only if the autosave path can't be made reliable — note the reason in this ticket if so).
- [ ] After typing into a field and triggering an event that previously reseeded `chapter_draft` (e.g. a coach run completing), the typed value is preserved (not reverted).
- [ ] Read-only stats (`word_count`, `voice_score`, `last_coached_at`) read from the same `ChapterMeta` instance as the editable fields — no drift between the two halves of the form.
- [ ] Any other form found in the audit that uses the same draft-and-Save pattern is converted to direct binding (or this ticket lists why it should stay staged, in the design notes).
- [ ] A short documented convention exists describing the pattern for future forms.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- The original reason `chapter_draft` exists (per `src/app/book.rs:398-433`) was likely to make "Save" atomic and to avoid persisting on every keystroke. Both can be addressed without a draft buffer:
  - **Atomicity**: `chapter.json` is small; an atomic write (write-tmp + rename) on debounce or on chapter-close is enough.
  - **Per-keystroke I/O**: debounce by 500–1000 ms after the last `changed()` event, and always flush on chapter switch / book close.
- If during the audit it turns out the entity inspector has a reason to stay staged (e.g. rename has multi-file ripple effects), keep that one and document it. The rule is "default to direct binding, opt out with a documented reason," not "no drafts allowed anywhere."
- This ticket touches state ownership; expect it to surface latent bugs in chapter-switch sequencing. Allocate generously.
