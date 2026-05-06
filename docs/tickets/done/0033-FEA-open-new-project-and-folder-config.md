# 0033 — FEA: Open / new project + per-book manuscript & authors folders

**Type:** FEATURE
**Created:** 2026-05-05
**Depends on:** none

## Problem
CkWriter is hardcoded to a single book today. The Open-book modal asks for a typed path and lists recent paths as plain links; there's no way to scaffold a fresh book in an empty directory from the app, and the only book the writer has ever opened (`The Redemption Chronicles`) bakes its two managed manuscript folders — `Ancient` and `Modern` — into a `pub const MANAGED_FOLDERS: &[&str]` in `src/book/manuscript.rs:29`. The same holds for character storage: `Info/Characters` is hardcoded into `EntityKind::folder()` (`src/book/entity.rs:19`). A second book with a different layout (one folder named `Manuscript`, characters split across `Info/Characters` and `Info/Personae`) cannot be opened — the file tree, normalize pass, add-chapter dialog, and entity loader all silently filter it to nothing. Cost of *not* doing this: the writer can't start a second book without forking the binary, and any writer who picks up the tool has to reverse-engineer the folder names before it'll do anything useful.

## Scope
- **Per-book folder config (`src/book/mod.rs::BookConfig`).** Add two fields, both serialised into `Info/index.json`:
  - `manuscript_folders: Vec<String>` — ordered list of folders under the book root that contain `.tex` chapter files. Replaces `manuscript::MANAGED_FOLDERS`.
  - `authors_folders: Vec<String>` — ordered list of folders (relative to the book root, e.g. `Info/Characters`) that contain character entity JSONs.
  - Both default-on-deserialize so existing `Info/index.json` files (no fields) keep working. Defaults: `manuscript_folders = ["Ancient", "Modern"]` for backward compat with the writer's open book; `authors_folders = ["Info/Characters"]` for the same reason.
- **Thread the manuscript folders through the pipeline.** Drop `MANAGED_FOLDERS` from `manuscript.rs`. Every reader (`chapters::normalize`, `chapters::list_folders`, `chapters::add_chapter`'s validation, `book::load_or_seed_manuscript`, `app::NewChapterDialog::default`, `ui::file_tree`) takes a `&[String]` (or `&BookConfig`) instead. The previous `chapters::*` tests that build a temp dir with `Ancient`/`Modern` keep passing by passing those names explicitly.
- **Thread the authors folders through `Entities::load`.** For `EntityKind::Character`, iterate `authors_folders` instead of `Info/Characters`. `Entity::file_path` uses the first entry of `authors_folders` as the canonical save target. `EntityKind::Location | Event | Timeline` keep their hardcoded folders for v1 — out of scope below.
- **Project scaffolder (`src/book/init.rs`, new module).** `pub fn init_project(root: &Path) -> Result<()>`:
  - Refuses if `root` doesn't exist, isn't a directory, or contains any visible entries (dotfiles allowed — git, IDE crud).
  - Writes `main.tex` (minimal `\documentclass{book} … \begin{document}\end{document}` with the sentinel-wrapped empty include block), `Info/manuscript.json` (`{"chapters":[]}`), and `Info/index.json` (`BookConfig` with `manuscript_folders = []`, `authors_folders = ["Info/Characters"]`, plus the existing `main_tex`/`voice_prompt_file`/`roadmap_file` defaults). Creates `Info/Characters/` as an empty directory.
  - **Does not** pre-create `Ancient`/`Modern` — those are project-specific. The user adds folders via the settings dialog (next bullet).
- **Welcome / book picker (`app::book_picker` in `src/app/mod.rs`).** Repurpose the existing modal:
  - Headline: "Open or create a CkWriter book".
  - Recent section: each entry in `settings.recent_books` rendered as a button. Path text is `theme::TEXT_MUTED`-small under the directory name.
  - Open section: existing path textbox + Open button, unchanged behaviour.
  - New project section: a second path textbox + "New project here" button. Click calls `init_project(&path)` then `open_book(&path)`. Errors render inline in the modal (e.g. "directory not empty") instead of dropping into `last_error`.
- **Per-project folder editor (`src/ui/settings_dialog.rs`).** When a book is open, append a "Project" section at the bottom of the modal with two editors:
  - Manuscript folders: list of strings with a `+` button to add a new row, `–` per row to remove, and a textbox per row. Order is preserved (drag-to-reorder is out of scope; add/remove is enough for v1).
  - Authors folders: same widget, separate list.
  - Save writes `Info/index.json` and re-runs `Book::open`'s file-tree + entity rebuild so the sidebar reflects the change immediately. (The simplest implementation: re-call `app.open_book(&book.root)` on save — see Design notes for the dirty-form interaction.)
- **Tests.**
  - `init_project` writes the expected scaffold and refuses non-empty dirs.
  - `chapters::normalize` and `chapters::list_folders` honour an arbitrary `manuscript_folders` slice (not just `Ancient`/`Modern`).
  - `Entities::load` reads characters from a custom `authors_folders` list (multi-folder).
  - `BookConfig` deserializes a file missing both new fields and produces the back-compat defaults.

## Out of scope
- Locations / Events / Timeline folder configurability. They stay at their hardcoded paths in `EntityKind::folder()`. Follow-up ticket if the writer ever needs it.
- Native file-picker dialog (e.g. `rfd`). v1 keeps the textbox path UX — no new dependency.
- Drag-to-reorder for the folder lists in settings.
- Migration of an existing book whose `Info/index.json` *does* have the new fields but they're empty: that's a user choice, we honour it. (`Info/index.json` is a contract file, not a generated one.)
- Project metadata (`\title{}`, `\author{}`) editor in settings — distinct concept from "authors folders" despite the name overlap.
- Templates beyond the empty scaffold (no "epic fantasy template" or similar).

## Acceptance criteria
- [x] A new `init_project(&Path)` API scaffolds an empty directory with `main.tex`, `Info/manuscript.json`, `Info/index.json`, and `Info/Characters/`. Refuses non-empty dirs (visible entries only).
- [x] `BookConfig` carries `manuscript_folders: Vec<String>` and `authors_folders: Vec<String>`, both with serde defaults that match the existing book's layout (`["Ancient","Modern"]` and `["Info/Characters"]`). The current book opens unchanged.
- [x] `MANAGED_FOLDERS` const is removed from `src/book/manuscript.rs`. All call sites read the configured list.
- [x] `Entities::load` reads characters from every directory listed in `authors_folders`. New characters are saved into the first entry. The other three entity kinds are unaffected.
- [x] The book picker modal exposes: a recents list (buttons), an "Open existing" path field, and a "New project here" path field with its own button. The new-project button creates the scaffold and opens it.
- [x] When a book is open, the settings dialog has a "Project" section with editable lists for manuscript and authors folders. Saving re-loads the book so the sidebar and entity store reflect the change.
- [x] `cargo clippy --all-targets --all-features` and `cargo test --all-targets --all-features` clean — 0 warnings, 0 errors.

## Design notes
- `BookConfig` already lives in `src/book/mod.rs`; the new fields fit there. Keep them `#[serde(default = "...")]` so older `Info/index.json` files load. The defaults intentionally encode the writer's *current* layout, so the existing `TheRedemptionChronicles` book opens with no on-disk changes.
- The `MANAGED_FOLDERS` const is referenced from 11 sites today (search: `MANAGED_FOLDERS`). Plumbing a `&[String]` through all of them is mechanical; the awkward case is `chapters::add_chapter` which is called from `app::add_chapter`. Pass `&book.config.manuscript_folders` from there. The chapter-ops tests pass an explicit slice.
- `Entity::file_path(&self, root)` doesn't know the config. Resolve by changing the signature to `file_path(&self, root, authors_folders)` for `Character`, leaving the other kinds alone. Or simpler: keep the signature, store the canonical save folder on `Entities` itself (loaded once at `Book::open`). The latter avoids touching every save site; preferred.
- The file tree (`src/ui/file_tree.rs`) uses `MANAGED_FOLDERS` to decide which sidebar headings to draw under "Manuscript". It needs the per-book list too — pass it via `app.book.as_ref()?.config.manuscript_folders`.
- "Empty directory" check: `fs::read_dir(root)` filtered to non-dotfile entries. `.git`, `.gitignore`, `.idea/`, etc. are tolerated so a writer can `git init` first.
- The settings dialog's project-folder editor saves through a new `BookConfig::save(&Path)` method (Info/index.json round-trip). The existing dialog already runs settings autosave on close; the project-folder save runs immediately on `+`/`–`/edit so a typo can't be lost.
- Re-loading the book on project-folder save: `open_book` already gates on dirty forms via `request_open_book_picker`. For settings, the simplest correct path is to clear `chapter_form` / `entity_form` *before* calling `open_book` again — which is fine because saving the project folders is itself an explicit "I've finished settings" act, distinct from chapter editing. If a chapter is dirty, save it first (same as the rest of settings autosave does implicitly).
- Welcome modal has two textboxes; default-focus the recents list when it's non-empty, else the "Open existing" field. Don't autofocus the "New project" field — it's the less common path.
