# 0027 — FEA: Author notes — per-paragraph guidance + per-dismissal reason

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0025

## Problem
Two adjacent gaps in how the writer talks to the AI:

1. **No way to give the model paragraph-level guidance.** "This paragraph is supposed to read flat — the character is exhausted, prose should be too" is the kind of intent the model needs in order to stop second-guessing. Today the writer can only dismiss flags one by one; there's no way to pre-empt them with a note.

2. **No way to record *why* a flag was dismissed.** Today's "Already reviewed — do not flag again" prompt section (#0025) sends just the quote. The model sees what to skip but not why; it can't generalize. Recording the rationale ("colloquial register is intentional", "callback to chapter 1") and feeding it back is what lets the AI stop repeating the same mistakes across runs.

Both are author-side annotations the AI consumes; neither exists today.

## Scope

### 1. Per-paragraph notes

- `src/book/chapter.rs` (or wherever `ChapterMeta` lives) — add `paragraph_notes: BTreeMap<String, String>` on `ChapterMeta`, keyed by paragraph_id. `#[serde(default)]` so legacy chapter-meta files deserialize as empty.
- `src/llm/prompts.rs::build_user_with_history` — extend signature with `paragraph_note: Option<&str>`. Non-empty notes render as a section titled `## Author guidance for this paragraph` above the existing "Already reviewed" section, with framing wording instructing the model to treat the note as the author's stated intent for the paragraph and to avoid flagging prose consistent with it. Empty/None collapses to today's output byte-for-byte.
- `src/app/coach.rs` — pass the focused paragraph's note into `build_user_with_history` from both the per-paragraph and chapter-walk run paths.
- `src/ui/scope_panel.rs` — editable textbox row for the focused paragraph's note, anchored to whichever paragraph the editor cursor is in. Uses the existing forms-framework draft + dirty + Save plumbing. Save persists `ChapterMeta` via the existing chapter-write path.

### 2. Per-dismissal reason

- `src/book/suggestions.rs` — add `dismissal_note: Option<String>` to `SuggestionRecord`. `#[serde(default)]`.
- `src/llm/prompts.rs::build_user_with_history` — in the "Already reviewed" section, append each record's `dismissal_note` to its quote line: `- "<quote>" — dismissed because: <note>`. Records with no note keep today's `- "<quote>"` form.
- `src/ui/scope_panel.rs::revision_card` — for Dismissed cards, render an editable textbox + Save button for the dismissal reason. Forms-framework plumbing.
- Dismiss stays single-click as today; the note is filled in retroactively on the dismissed card. The AI prompt re-renders on every run, so back-filled notes take effect at the next coach run on that paragraph.

### Tests
- `dismissal_note_round_trips_through_chapter_store` — serialize → deserialize preserves the note; legacy data → None.
- `paragraph_notes_round_trip` — serialize → deserialize preserves the map; missing field deserializes as empty.
- `paragraph_note_renders_in_prompt` — non-empty note appears under "Author guidance for this paragraph"; empty/None produces today's output byte-for-byte.
- `dismissal_note_renders_alongside_quote` — record with note renders as `"<quote>" — dismissed because: <note>`; record without note renders as `"<quote>"`.

## Out of scope
- Editor-gutter indicator for paragraphs that carry notes. v1 surfaces notes only in the scope panel; a "note present" gutter glyph is a follow-up if the writer wants it.
- Per-card notes on *non-dismissed* flags (Proposed, Accepted). Only Dismissed records get the note field in v1.
- Manual "same as..." flag-duplicate resolution (the original #0027 scope) — dropped because feeding the AI dismissal *reasons* gives the model rationale to generalize without an explicit alias list. Cheaper code, richer prompt signal.
- Cross-chapter or book-level author notes. Paragraph- and flag-scoped only.
- Markdown rendering inside notes. Plain text in v1.
- Note-orphaning UX when a paragraph's id rotates (a content edit big enough to defeat Jaccard 0.5). v1 accepts the trade-off; rebind is a follow-up if it bites.

## Acceptance criteria
- [x] Editable per-paragraph notes textbox visible in the scope panel for the focused paragraph; edits enter dirty state; Save persists to `ChapterMeta`; revert restores last-saved value. _(writer-confirmed 2026-05-03)_
- [x] After Save, the next coach run on that paragraph includes the note in the rendered prompt under `## Author guidance for this paragraph`. _(writer-confirmed 2026-05-03; pinned by `paragraph_note_renders_above_already_reviewed` in `src/llm/prompts.rs`)_
- [x] Each Dismissed coach card shows a textbox + Save for the dismissal reason; edits enter dirty state; Save persists to the chapter suggestion store. _(writer-confirmed 2026-05-03)_
- [x] After Save, the next coach run on that paragraph includes the dismissal note alongside its quote in the "Already reviewed" section. _(writer-confirmed 2026-05-03; pinned by `dismissal_note_renders_alongside_quote` in `src/llm/prompts.rs`)_
- [x] `paragraph_notes` (on `ChapterMeta`) and `dismissal_note` (on `SuggestionRecord`) round-trip through their respective JSON files; legacy files (no field) deserialize as empty / None. _(round-trip + legacy-load tests in `src/book/chapter_meta.rs` and `src/book/suggestions.rs`)_
- [x] `cargo clippy --all-targets -- -D warnings` and `cargo test` clean. _(174 passed, 0 warnings)_

## Design notes
- Both note paths reuse the forms framework (draft + dirty + explicit Save, no autosave, no Cmd+S). Matches the writer's standing preference and protects long textboxes from saving mid-typo.
- Persistence: paragraph notes ride on `ChapterMeta` (one file per chapter); dismissal notes ride on `SuggestionRecord` in the existing per-chapter suggestion store. No new files, no new directories.
- Why dismissal notes are retroactive (not a dialog at dismiss-click): keeps the Dismiss action single-click. The AI prompt re-renders every run, so a note added an hour after dismissal still takes effect at the next coach run.
- Why we drop the "Same as..." dedup originally scoped here: feeding the AI dismissal reasons gives the model rationale to generalize without an explicit alias list. Cheaper code, richer prompt signal, fewer moving parts. If paraphrase-dups still surface despite reasons being fed back, file follow-up.
- Per-paragraph notes vs. per-flag notes: both feed the same prompt builder, but the paragraph note is *intent* (proactive: "this is how it should read") and the dismissal note is *correction* (reactive: "you got it wrong this way"). Keeping them as two distinct sections in the prompt — `## Author guidance for this paragraph` vs. `## Already reviewed` — preserves that semantic split for the model.
