use crate::book::manuscript::ChapterRef;
use crate::book::paragraphs::{self, ParagraphMeta};
use crate::book::{chapters as chapter_ops, Book, Chapter};
use crate::extract::EntityMatcher;
use std::path::Path;

impl super::CkWriterApp {
    pub fn open_book(&mut self, root: &Path) -> anyhow::Result<()> {
        let book = Book::open(root)?;
        let matcher = EntityMatcher::build(&book.entities);
        self.expanded_dirs.clear();
        match self.settings.expanded_dirs.get(&book.root) {
            Some(saved) if !saved.is_empty() => {
                self.expanded_dirs.extend(saved.iter().cloned());
            }
            _ => {
                // First-time open: pre-expand top-level dirs so the sidebar isn't
                // a wall of collapsed folders.
                self.expanded_dirs.insert(book.root.clone());
                for child in &book.file_tree.children {
                    if child.is_dir {
                        self.expanded_dirs.insert(child.path.clone());
                    }
                }
            }
        }
        self.book = Some(book);
        self.matcher = Some(matcher);
        self.current_chapter = None;
        self.editor_text.clear();
        self.entity_hits.clear();
        self.current_paragraphs.clear();
        self.revisions.clear();
        self.selected_revision = None;
        self.dirty = false;
        self.selected_entity = None;
        self.entity_dirty = None;
        self.chapter_draft = None;
        self.char_stream = None;
        self.last_char_buffer = None;
        self.char_proposals.clear();
        self.char_extract_error = None;
        self.progression_stream = None;
        self.progression_target = None;
        self.progression_status = None;
        self.char_index = None;
        self.chat_messages.clear();
        self.chat_input.clear();
        self.chat_stream = None;
        self.chat_pending_assistant.clear();
        self.chat_error = None;
        self.chat_chapter = None;
        self.rebuild_char_index();
        self.settings.touch_recent(root);
        if let Some(model) = self.book.as_ref().and_then(|b| b.config.model.clone()) {
            self.settings.model = model;
        }
        let _ = self.settings.save();

        // Auto-open last chapter if it lives in this book.
        if let Some(last) = self.settings.last_chapter.clone() {
            if last.starts_with(root) && last.exists() {
                self.open_chapter(&last);
            }
        }
        Ok(())
    }

    pub fn open_chapter(&mut self, path: &Path) {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let chapter_changed = self
            .current_chapter
            .as_ref()
            .map(|c| c.file_path != path)
            .unwrap_or(true);
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.editor_text = text;
                self.dirty = false;
                if chapter_changed {
                    self.reset_chat();
                }
                if let Some(place) = self.settings.chapter_places.get(path) {
                    self.pending_cursor_char = Some(place.cursor);
                    self.pending_scroll_offset = Some(place.scroll);
                } else {
                    self.pending_cursor_char = None;
                    self.pending_scroll_offset = None;
                }
                if let Some(book) = &self.book {
                    self.current_chapter = book.chapter_by_path(path).cloned().or_else(|| {
                        let include_path = path
                            .strip_prefix(&book.root)
                            .ok()
                            .and_then(|p| p.with_extension("").to_str().map(str::to_string))
                            .unwrap_or_else(|| path.display().to_string());
                        let folder = include_path
                            .split_once('/')
                            .map(|(f, _)| f.to_string())
                            .unwrap_or_default();
                        let stem = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled");
                        let name = crate::book::manuscript::strip_number_prefix(stem).to_string();
                        Some(Chapter {
                            folder,
                            name,
                            include_path,
                            file_path: path.to_path_buf(),
                            display_title: stem.to_string(),
                            in_manuscript: false,
                            meta: crate::book::chapter_meta::ChapterMeta::default(),
                        })
                    });
                }
                self.refresh_entity_hits();
                self.revisions.clear();
                self.selected_revision = None;
                self.last_error = None;
                self.diff_baseline = None;
                self.diff_baseline_chapter = None;
                self.diff_baseline_error = None;
                self.diff_scroll_y = 0.0;
                self.settings.last_chapter = Some(path.to_path_buf());
                let _ = self.settings.save();
                self.seed_chapter_draft();
            }
            Err(e) => {
                self.last_error = Some(format!("open failed: {e}"));
            }
        }
    }

    pub fn save_chapter(&mut self) -> anyhow::Result<()> {
        let Some(ch) = &self.current_chapter else {
            return Ok(());
        };
        let file_path = ch.file_path.clone();
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        std::fs::write(&file_path, &self.editor_text)?;
        self.dirty = false;
        // Recompute word_count + paragraph index from the just-saved buffer.
        // Both are cheap (single regex pass + a normalize-and-hash sweep) and
        // keep the persisted view in lockstep with the file.
        if !folder.is_empty() && !name.is_empty() {
            let prose = crate::book::latex::to_prose(&self.editor_text);
            let wc = crate::book::chapter_meta::word_count_from_prose(&prose);
            let parsed = paragraphs::parse_and_match(&self.editor_text, &self.current_paragraphs);
            let new_meta: Vec<ParagraphMeta> = parsed.iter().map(|p| p.meta()).collect();
            self.current_paragraphs = parsed;
            self.update_chapter_meta(&folder, &name, |m| {
                m.word_count = wc;
                m.paragraphs = new_meta;
            });
        }
        // Cross-chapter index reads from disk; refresh it so the just-saved
        // chapter's edits are reflected in the inspector's "Appears in" list.
        self.rebuild_char_index();
        Ok(())
    }

    /// Apply `mutate` to the chapter's metadata, persist it to disk, and
    /// mirror the change onto `current_chapter.meta` so the UI reads the
    /// fresh value without a reload.
    pub(crate) fn update_chapter_meta<F: FnOnce(&mut crate::book::chapter_meta::ChapterMeta)>(
        &mut self,
        folder: &str,
        name: &str,
        mutate: F,
    ) {
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let Some(idx) = book
            .chapters
            .iter()
            .position(|c| c.folder == folder && c.name == name)
        else {
            return;
        };
        mutate(&mut book.chapters[idx].meta);
        let meta = book.chapters[idx].meta.clone();
        let root = book.root.clone();
        if let Err(e) = crate::book::chapter_meta::save(&root, folder, name, &meta) {
            log::warn!("chapter meta save {folder}/{name} failed: {e}");
        }
        if let Some(cur) = self.current_chapter.as_mut() {
            if cur.folder == folder && cur.name == name {
                cur.meta = meta;
            }
        }
    }

    /// Add a new chapter under `folder` with `title`. Saves any pending
    /// editor edits first so the chapter ops module never has to worry
    /// about the in-flight buffer. Opens the new chapter for editing.
    pub fn add_chapter(&mut self, folder: &str, title: &str) -> anyhow::Result<()> {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let Some(book) = self.book.as_mut() else {
            return Err(anyhow::anyhow!("no book open"));
        };
        let main_tex_name = book.config.main_tex.clone();
        let entry = chapter_ops::add_chapter(
            &book.root.clone(),
            &main_tex_name,
            &mut book.manuscript,
            folder,
            title,
        )?;
        book.reload_chapters();
        let target_path = book
            .chapters
            .iter()
            .find(|c| c.folder == entry.folder && c.name == entry.name)
            .map(|c| c.file_path.clone());
        if let Some(p) = target_path {
            self.open_chapter(&p);
        }
        Ok(())
    }

    /// Delete a chapter file and remove it from the manuscript. If the
    /// deleted chapter is currently open, clears the editor.
    pub fn delete_chapter(&mut self, folder: &str, name: &str) -> anyhow::Result<()> {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let Some(book) = self.book.as_mut() else {
            return Err(anyhow::anyhow!("no book open"));
        };
        let main_tex_name = book.config.main_tex.clone();
        let root = book.root.clone();
        chapter_ops::delete_chapter(&root, &main_tex_name, &mut book.manuscript, folder, name)?;
        book.reload_chapters();
        if let Some(ch) = self.current_chapter.as_ref() {
            if ch.folder == folder && ch.name == name {
                self.current_chapter = None;
                self.editor_text.clear();
                self.entity_hits.clear();
                self.current_paragraphs.clear();
            }
        }
        Ok(())
    }

    /// Drop a chapter from the manuscript without deleting its file.
    pub fn exclude_chapter(&mut self, folder: &str, name: &str) -> anyhow::Result<()> {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let Some(book) = self.book.as_mut() else {
            return Err(anyhow::anyhow!("no book open"));
        };
        let main_tex_name = book.config.main_tex.clone();
        let root = book.root.clone();
        chapter_ops::exclude_chapter(&root, &main_tex_name, &mut book.manuscript, folder, name)?;
        book.reload_chapters();
        // The current chapter's filename just changed (number stripped); resync
        // its file_path so subsequent saves go to the right file.
        self.resync_current_chapter();
        Ok(())
    }

    /// Append an existing orphan back into the manuscript.
    pub fn include_chapter(&mut self, folder: &str, name: &str) -> anyhow::Result<()> {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let Some(book) = self.book.as_mut() else {
            return Err(anyhow::anyhow!("no book open"));
        };
        let main_tex_name = book.config.main_tex.clone();
        let root = book.root.clone();
        chapter_ops::include_chapter(&root, &main_tex_name, &mut book.manuscript, folder, name)?;
        book.reload_chapters();
        self.resync_current_chapter();
        Ok(())
    }

    /// Replace the manuscript order. Renumbers files per folder; the open
    /// chapter's file_path is updated to its new path on disk.
    pub fn reorder_manuscript(&mut self, new_order: Vec<ChapterRef>) -> anyhow::Result<()> {
        if self.dirty {
            let _ = self.save_chapter();
        }
        let Some(book) = self.book.as_mut() else {
            return Err(anyhow::anyhow!("no book open"));
        };
        let main_tex_name = book.config.main_tex.clone();
        let root = book.root.clone();
        chapter_ops::reorder_manuscript(&root, &main_tex_name, &mut book.manuscript, new_order)?;
        book.reload_chapters();
        self.resync_current_chapter();
        Ok(())
    }

    /// After a manuscript op renames the open chapter's file, look it up by
    /// (folder, name) — its identity — and refresh `current_chapter` to the
    /// new path. Without this, Cmd+S would write to a path that no longer
    /// exists and the diff view would chase stale baselines.
    fn resync_current_chapter(&mut self) {
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return;
        };
        let Some(book) = self.book.as_ref() else {
            return;
        };
        if let Some(found) = book
            .chapters
            .iter()
            .find(|c| c.folder == ch.folder && c.name == ch.name)
        {
            self.current_chapter = Some(found.clone());
            self.diff_baseline = None;
            self.diff_baseline_chapter = None;
            self.diff_baseline_error = None;
        } else {
            // Chapter no longer exists (deleted upstream); clear.
            self.current_chapter = None;
            self.editor_text.clear();
            self.entity_hits.clear();
            self.current_paragraphs.clear();
        }
    }

    /// Lazily load the `HEAD` baseline for the current chapter. Cached by
    /// chapter file path; cleared by `open_chapter`. Idempotent — safe to
    /// call every frame from the diff view.
    pub fn ensure_diff_baseline(&mut self) {
        let Some(ch) = self.current_chapter.as_ref() else {
            self.diff_baseline = None;
            self.diff_baseline_chapter = None;
            self.diff_baseline_error = None;
            return;
        };
        if self.diff_baseline_chapter.as_deref() == Some(ch.file_path.as_path()) {
            return;
        }
        let path = ch.file_path.clone();
        match crate::diff::head_baseline(&path) {
            Ok(Some(text)) => {
                self.diff_baseline = Some(text);
                self.diff_baseline_error = None;
            }
            Ok(None) => {
                self.diff_baseline = None;
                self.diff_baseline_error =
                    Some("no HEAD baseline (file is untracked or new)".into());
            }
            Err(e) => {
                self.diff_baseline = None;
                self.diff_baseline_error = Some(format!("git: {e}"));
            }
        }
        self.diff_baseline_chapter = Some(path);
    }

    pub fn refresh_entity_hits(&mut self) {
        self.entity_hits = match &self.matcher {
            Some(m) => m.find(&self.editor_text),
            None => Vec::new(),
        };
    }

    pub fn run_import(&mut self) {
        let Some(book) = &mut self.book else { return };
        let root = book.root.clone();
        match crate::import::import_personae(&root) {
            Ok(n) => {
                self.import_status = Some(format!("imported {n} characters from Personae.txt"));
                book.reload_entities();
                self.matcher = Some(EntityMatcher::build(&book.entities));
                self.refresh_entity_hits();
                self.rebuild_char_index();
            }
            Err(e) => {
                self.import_status = Some(format!("import failed: {e}"));
            }
        }
    }

    /// Initialise (or replace) the chapter-tab draft from the current
    /// chapter's saved metadata, and rebuild `current_paragraphs` from the
    /// open file. Called whenever the active chapter changes — any unsaved
    /// edits in the previous draft are discarded, mirroring how the legacy
    /// `.notes.md` scratchpad worked.
    pub fn seed_chapter_draft(&mut self) {
        let Some(ch) = self.current_chapter.as_ref() else {
            self.chapter_draft = None;
            self.current_paragraphs.clear();
            return;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        let prior_meta = ch.meta.paragraphs.clone();
        self.chapter_draft = Some(crate::app::ChapterDraft {
            folder: folder.clone(),
            name: name.clone(),
            summary: ch.meta.summary.clone(),
            goals: ch.meta.goals.clone(),
            plot_notes: ch.meta.plot_notes.clone(),
            dirty: false,
        });

        // Build the runtime paragraph index against the on-disk index. If the
        // file's been edited outside the app (or this is the first open after
        // adding the field), the result will differ — persist it so future
        // opens see a stable index.
        let parsed = paragraphs::parse_and_match_meta(&self.editor_text, &prior_meta);
        let needs_save = paragraphs::differs(&parsed, &prior_meta);
        let new_meta: Vec<ParagraphMeta> = parsed.iter().map(|p| p.meta()).collect();
        self.current_paragraphs = parsed;
        if needs_save && !folder.is_empty() && !name.is_empty() {
            self.update_chapter_meta(&folder, &name, |m| m.paragraphs = new_meta);
        }
    }

    /// Persist the chapter-tab draft into the chapter's metadata file. No-op
    /// if there is no draft, no current chapter, or the draft belongs to a
    /// different chapter than the one currently open (defensive — happens if
    /// chapter switching races with a save shortcut).
    pub fn save_chapter_draft(&mut self) {
        let Some(draft) = self.chapter_draft.clone() else {
            return;
        };
        let cur_matches = self
            .current_chapter
            .as_ref()
            .is_some_and(|c| c.folder == draft.folder && c.name == draft.name);
        if !cur_matches {
            return;
        }
        self.update_chapter_meta(&draft.folder, &draft.name, |m| {
            m.summary = draft.summary.clone();
            m.goals = draft.goals.clone();
            m.plot_notes = draft.plot_notes.clone();
        });
        if let Some(d) = self.chapter_draft.as_mut() {
            d.dirty = false;
        }
    }

    /// Scroll the editor so the byte at `byte_start` is in view, and place
    /// the cursor there. Used by the AI panel's selected-card flow so the
    /// writer can see a flagged passage in context before accepting it.
    /// The actual scroll happens after the editor renders, using the galley's
    /// pixel-accurate cursor rect; we cannot use line counting because LaTeX
    /// prose paragraphs wrap onto many visual rows per source line.
    pub fn jump_to_anchor(&mut self, byte_start: usize) {
        let cap = byte_start.min(self.editor_text.len());
        let char_idx = self.editor_text[..cap].chars().count();
        log::info!(
            "jump_to_anchor: byte={byte_start} cap={cap} char_idx={char_idx} text_len={}",
            self.editor_text.len(),
        );
        self.pending_cursor_char = Some(char_idx);
        self.pending_scroll_to_cursor = true;
        // These two paths drive line- or pixel-based scrolls and would race
        // with the post-render scroll-to-cursor below.
        self.pending_scroll_line = None;
        self.pending_scroll_offset = None;
    }

    pub fn jump_to_source(&mut self, file: &Path, line: u32) {
        self.read_mode = false;
        let needs_open = self
            .current_chapter
            .as_ref()
            .map(|c| c.file_path != file)
            .unwrap_or(true);
        if needs_open {
            self.open_chapter(file);
        }
        self.pending_scroll_line = Some(line.saturating_sub(1) as usize);
    }
}
