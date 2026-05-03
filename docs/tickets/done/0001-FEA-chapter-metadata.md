# 0001 — FEA: Chapter metadata + inspector panel

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** none

## Problem
Chapters today are just `.tex` files plus a `(folder, name)` entry in `manuscript.json`. There is no place to store per-chapter metadata: summary, goals, plot notes, POV, computed word count, last voice score. Every later AI feature would otherwise invent its own per-chapter storage. Writers also have nowhere to record discovery-writing notes that aren't manuscript prose.

This ticket is the foundation that #0002–#0006 hang off.

## Scope
- New file `<book>/Info/chapters/<folder>/<name>.json`, keyed by stable CamelCase `name` (mirrors the `.tex` layout, no number prefix)
- New module `src/book/chapter_meta.rs` with:
  - `ChapterMeta { summary, goals, plot_notes, pov: Option<EntityId>, tags: Vec<String>, word_count: usize, voice_score: Option<i32>, last_coached_at: Option<i64> }`
    - `voice_score` is `i32` (not `u32`) to match `parse_voice`'s existing return type — avoids a needless conversion
    - `pov` and `tags` are storage-only in v1; #0002–#0006 will write to them. No widgets this ticket.
  - `load(root, folder, name)`, `save(root, folder, name, &meta)`
- `Chapter` struct gains a `meta: ChapterMeta` field, populated in `build_chapters`
- `save_chapter` recomputes and persists `word_count` from prose
- Voice pipeline ingest writes `voice_score` and `last_coached_at` (the score is currently parsed by `parse_voice` and discarded by `ingest_response`)
- Right-panel **Chapter tab** (replaces the existing **Notes** tab):
  - Editable: `summary`, `goals`, `plot_notes` (the latter absorbs the old `.notes.md` scratchpad — same free-form purpose)
  - Read-only: `word_count`, `voice_score`, `last_coached_at`
- Migration on first open:
  - Missing `chapter.json` is silently auto-created with `word_count` filled, rest empty
  - If a `<chapter>.notes.md` exists alongside the `.tex`, its contents are migrated into `plot_notes` and the file is left in place (user keeps a copy in git history; new edits go through `chapter.json`)
  - Malformed JSON falls back to defaults with a warning, like `manuscript.json`

## Out of scope
- Paragraph index — that's #0002
- POV picker UI of any kind (text input or dropdown) — storage only in v1
- Tags UI — storage only in v1
- Plot/scene structure beyond free-text
- Bulk view of "all chapters with their metadata" — separate ticket if wanted
- Deleting orphan `.notes.md` files after migration — leave them in place; `chapter.json` is now the source of truth

## Acceptance criteria
- [x] Opening a book auto-creates `chapter.json` for every chapter (manuscript + orphan in managed folders) that doesn't have one
- [x] Existing `<chapter>.notes.md` content is migrated into `plot_notes` on first open
- [x] Word count visible in the Chapter tab; updates when `save_chapter` runs
- [x] Editing summary/goals/plot_notes in the Chapter tab persists across reopen
- [x] Voice pipeline writes `voice_score` and `last_coached_at` after a successful run; visible in the Chapter tab
- [x] Renumbering or reordering chapters preserves metadata (keyed by stable `name`)
- [x] Old Notes tab and `notes_text`/`notes_path`/`load_notes`/`save_notes` plumbing is removed (no dead code left behind)
- [x] Unit tests: load/save roundtrip, missing-file migration, malformed-file fallback, `.notes.md` migration into `plot_notes`
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors)

## Design notes
- File path: `Info/chapters/Modern/Awakening.json` for `Modern/010_Awakening.tex`. The number prefix is renumber-derived; metadata uses the stable `name` only.
- Failure to load malformed `chapter.json`: log warning, return defaults (don't crash).
- `voice_score` extraction: `parse_voice` already returns a `score` field; only `ingest_response` change is to write it through to `book.chapter_meta(name).voice_score`.
- This ticket is the schema container; #0002 will add a `paragraphs: Vec<ParagraphMeta>` field to the same file.
