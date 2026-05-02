use crate::book::{Book, Chapter};
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
        self.revisions.clear();
        self.selected_revision = None;
        self.dirty = false;
        self.selected_entity = None;
        self.entity_dirty = None;
        self.notes_text.clear();
        self.notes_dirty = false;
        self.notes_path = None;
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
                        Some(Chapter {
                            include_path: path
                                .strip_prefix(&book.root)
                                .ok()
                                .and_then(|p| p.with_extension("").to_str().map(str::to_string))
                                .unwrap_or_else(|| path.display().to_string()),
                            file_path: path.to_path_buf(),
                            display_title: path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Untitled")
                                .to_string(),
                            in_manuscript: false,
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
                self.load_notes();
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
        std::fs::write(&ch.file_path, &self.editor_text)?;
        self.dirty = false;
        // Cross-chapter index reads from disk; refresh it so the just-saved
        // chapter's edits are reflected in the inspector's "Appears in" list.
        self.rebuild_char_index();
        Ok(())
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

    fn load_notes(&mut self) {
        let Some(ch) = &self.current_chapter else {
            return;
        };
        let p = ch.file_path.with_extension("notes.md");
        self.notes_text = std::fs::read_to_string(&p).unwrap_or_default();
        self.notes_dirty = false;
        self.notes_path = Some(p);
    }

    pub fn save_notes(&mut self) {
        let Some(p) = self.notes_path.clone() else {
            return;
        };
        if let Err(e) = std::fs::write(&p, &self.notes_text) {
            self.last_error = Some(format!("notes save: {e}"));
            return;
        }
        self.notes_dirty = false;
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
