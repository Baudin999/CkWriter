use crate::book::entity::{mirror_diff, slugify, Entity, EntityKind, MirrorOp, Relation};
use crate::book::{latex, Book, Chapter};
use crate::extract::{EntityHit, EntityMatcher};
use crate::index::CrossChapterIndex;
use crate::llm;
use crate::llm::characters::{ProposalStatus, ProposedCharacter};
use crate::llm::prompts::Pipeline;
use crate::llm::revision::Revision;
use crate::pdf;
use crate::scope;
use crate::settings::Settings;
use crate::theme;
use crate::ui;
use eframe::App;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    /// Parsed cleanly (strict or salvage); revisions populated, possibly empty.
    Done,
    /// Both strict parse and salvage produced zero flags. Caller may retry by
    /// asking the model to repair its own JSON.
    NeedsRepair,
}

pub struct CkWriterApp {
    pub settings: Settings,
    pub book: Option<Book>,
    pub matcher: Option<EntityMatcher>,
    pub current_chapter: Option<Chapter>,
    pub editor_text: String,
    pub dirty: bool,
    pub entity_hits: Vec<EntityHit>,

    pub scope_tab: ui::scope_panel::Tab,
    pub char_sub_tab: ui::scope_panel::CharSubTab,
    pub selected_entity: Option<String>,
    pub entity_dirty: Option<Entity>,
    /// Search filter shared by Cast / Personae master lists.
    pub character_search: String,

    pub stream: Option<llm::StreamHandle>,
    pub stream_pipeline: Option<Pipeline>,
    /// Set when `stream` is a follow-up call asking the model to repair the
    /// previous malformed JSON. Guarantees the repair fallback runs at most
    /// once per pipeline invocation — a failed repair surfaces an error
    /// rather than triggering another repair.
    pub stream_is_repair: bool,
    pub last_stream_buffer: Option<String>,
    pub revisions: Vec<Revision>,
    pub next_rev_id: u32,
    /// Which revision card the writer last clicked. Drives the editor jump,
    /// the highlighted-card style, and the stronger underline in-text.
    pub selected_revision: Option<u32>,

    /// Separate stream so character extraction never collides with the
    /// voice/show/prose coaching pipelines.
    pub char_stream: Option<llm::StreamHandle>,
    pub last_char_buffer: Option<String>,
    pub char_proposals: Vec<ProposedCharacter>,
    pub char_extract_error: Option<String>,

    /// Per-character progression run. Independent of char/coach streams so
    /// the writer can fire one without blocking other AI work.
    pub progression_stream: Option<llm::StreamHandle>,
    /// (entity_id, chapter_include_path) — what the in-flight stream is for.
    pub progression_target: Option<(String, String)>,
    pub progression_status: Option<String>,
    /// Cross-chapter occurrence index. Rebuilt lazily after entity edits or
    /// chapter saves so the inspector can show "Appears in N chapters".
    pub char_index: Option<CrossChapterIndex>,

    /// Conversational-AI panel state. The transcript holds user/assistant
    /// turns only; the system prompt (chapter prose + cast) is rebuilt on
    /// every send so it stays current with edits.
    pub chat_messages: Vec<llm::ChatMessage>,
    pub chat_input: String,
    pub chat_stream: Option<llm::StreamHandle>,
    pub chat_pending_assistant: String,
    pub chat_error: Option<String>,
    /// Chapter the chat history was started against. Used to invalidate the
    /// transcript when the writer opens a different chapter.
    pub chat_chapter: Option<PathBuf>,

    pub last_error: Option<String>,
    pub ollama_ok: bool,
    pub last_ollama_check: f64,
    pub available_models: Vec<String>,

    pub show_book_picker: bool,
    pub picker_path: String,
    /// Directory paths currently expanded in the file tree sidebar.
    pub expanded_dirs: HashSet<PathBuf>,
    pub show_settings: bool,
    pub settings_dirty: bool,
    pub last_settings_save: f64,
    pub import_status: Option<String>,

    pub notes_text: String,
    pub notes_dirty: bool,
    pub notes_path: Option<PathBuf>,

    pub read_mode: bool,
    pub pdf_building: bool,
    pub pdf_build_rx: Option<mpsc::Receiver<pdf::BuildOutcome>>,
    pub pdf_meta: Option<pdf::PdfMeta>,
    pub pdf_renderer: Option<pdf::PageRenderer>,
    pub pdf_textures: HashMap<u32, egui::TextureHandle>,
    pub pdf_error: Option<String>,
    pub pdf_dpi: u32,
    pub pending_scroll_line: Option<usize>,
    /// One-shot: pixel offset to apply to the editor ScrollArea on the next frame
    /// (used to restore the saved reading position when re-opening a chapter).
    pub pending_scroll_offset: Option<f32>,
    /// One-shot: char index to install into the editor's TextEditState on the
    /// next frame (used to restore the saved cursor when re-opening a chapter).
    pub pending_cursor_char: Option<usize>,
    /// One-shot: after the next editor render, scroll so the active cursor
    /// is in view. Uses the galley's pixel rect, which is the only correct
    /// way to position wrapped LaTeX paragraphs (line-counting on `\n` is
    /// wildly wrong because one source line wraps to many visual rows).
    pub pending_scroll_to_cursor: bool,
}

impl CkWriterApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let mut app = Self {
            settings,
            book: None,
            matcher: None,
            current_chapter: None,
            editor_text: String::new(),
            dirty: false,
            entity_hits: Vec::new(),
            scope_tab: ui::scope_panel::Tab::Characters,
            char_sub_tab: ui::scope_panel::CharSubTab::Cast,
            selected_entity: None,
            entity_dirty: None,
            character_search: String::new(),
            stream: None,
            stream_pipeline: None,
            stream_is_repair: false,
            last_stream_buffer: None,
            revisions: Vec::new(),
            next_rev_id: 1,
            selected_revision: None,
            char_stream: None,
            last_char_buffer: None,
            char_proposals: Vec::new(),
            char_extract_error: None,
            progression_stream: None,
            progression_target: None,
            progression_status: None,
            char_index: None,
            chat_messages: Vec::new(),
            chat_input: String::new(),
            chat_stream: None,
            chat_pending_assistant: String::new(),
            chat_error: None,
            chat_chapter: None,
            last_error: None,
            ollama_ok: false,
            last_ollama_check: 0.0,
            available_models: Vec::new(),
            show_book_picker: false,
            picker_path: String::new(),
            expanded_dirs: HashSet::new(),
            show_settings: false,
            settings_dirty: false,
            last_settings_save: 0.0,
            import_status: None,
            notes_text: String::new(),
            notes_dirty: false,
            notes_path: None,
            read_mode: false,
            pdf_building: false,
            pdf_build_rx: None,
            pdf_meta: None,
            pdf_renderer: None,
            pdf_textures: HashMap::new(),
            pdf_error: None,
            pdf_dpi: 144,
            pending_scroll_line: None,
            pending_scroll_offset: None,
            pending_cursor_char: None,
            pending_scroll_to_cursor: false,
        };
        if let Some(p) = app.settings.last_book.clone() {
            if p.exists() {
                if let Err(e) = app.open_book(&p) {
                    log::warn!("auto-open failed for {}: {e:#}", p.display());
                }
            }
        }
        if app.settings.recent_books.is_empty() {
            app.picker_path = "/home/baudin/Projects/TheRedemptionChronicles".into();
        } else {
            app.picker_path = app
                .settings
                .recent_books
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
        }
        app
    }

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
        if let Some(model) = self
            .book
            .as_ref()
            .and_then(|b| b.config.model.clone())
        {
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

    pub fn create_blank_entity(&mut self, kind: EntityKind) {
        let Some(book) = &mut self.book else { return };
        let mut n = 1usize;
        let mut id;
        loop {
            id = format!("new-{}-{n}", kind_singular_id(kind));
            if book.entity(&id).is_none() {
                break;
            }
            n += 1;
        }
        let e = Entity::new(kind, id.clone(), "New entity".to_string());
        let _ = book.save_entity(e);
        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
        self.selected_entity = Some(id);
    }

    pub fn commit_entity_edit(&mut self) {
        let Some(mut e) = self.entity_dirty.take() else { return };
        let Some(book) = &mut self.book else { return };
        // Re-slug if name changed and old id no longer matches.
        let original_id = e.id.clone();
        if e.id.is_empty() {
            e.id = slugify(&e.name);
        }
        if original_id != e.id {
            // remove old file
            if let Some(old) = book.entity(&original_id).cloned() {
                let _ = std::fs::remove_file(old.file_path(&book.root));
                book.entities.by_id.remove(&original_id);
            }
        }

        // Snapshot prior relations so we can mirror inverses on save.
        let prev_relations = book
            .entity(&original_id)
            .map(|p| p.relations.clone())
            .unwrap_or_default();
        // Drop relations whose target no longer exists or that point at self;
        // serde-default + manual edits to book.json could otherwise leave dangling pointers.
        e.relations.retain(|r| {
            let id = r.id.trim();
            !id.is_empty() && id != e.id && book.entity(id).is_some()
        });
        let new_relations = e.relations.clone();
        let saved_id = e.id.clone();

        if let Err(err) = book.save_entity(e) {
            self.last_error = Some(format!("save entity: {err}"));
            return;
        }

        let inverse_fn = |k: &str| book.data.inverse_relation(k);
        let ops = mirror_diff(&prev_relations, &new_relations, &saved_id, inverse_fn);
        for op in ops {
            if let Err(err) = apply_mirror_op(book, &saved_id, op) {
                log::warn!("mirror relation op failed: {err}");
            }
        }

        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
    }

    pub fn rebuild_char_index(&mut self) {
        let (Some(book), Some(matcher)) = (self.book.as_ref(), self.matcher.as_ref()) else {
            self.char_index = None;
            return;
        };
        let start = std::time::Instant::now();
        let chapter_count = book.chapters.len();
        let idx = CrossChapterIndex::build(book, matcher);
        log::info!(
            "char index rebuilt in {:?}: chapters={chapter_count} indexed_entities={} total_occurrences={}",
            start.elapsed(),
            idx.entity_count(),
            idx.total_occurrences_all(),
        );
        self.char_index = Some(idx);
    }

    /// Send the current chapter's prose to ollama and ask it to extract every
    /// named character. Result is parsed and diffed against the entity DB
    /// when the stream completes.
    pub fn extract_characters_from_chapter(&mut self) {
        if self.char_stream.is_some() {
            log::debug!("character extraction already in flight, ignoring re-trigger");
            return;
        }
        if self.book.is_none() {
            self.char_extract_error = Some("open a book first".into());
            return;
        }
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.char_extract_error = Some("nothing to extract from".into());
            return;
        }
        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "character extraction start: chapter={chapter_label:?} prose_chars={} model={}",
            prose.chars().count(),
            self.settings.model,
        );
        let messages = vec![
            llm::ChatMessage::system(llm::characters::SYSTEM_PROMPT.to_string()),
            llm::ChatMessage::user(llm::characters::build_user(&prose)),
        ];
        // Long chapters can run 90KB+ of prose. 32k tokens fits any chapter
        // we've seen and is well within gemma3's supported window.
        // num_predict is generous because a single chapter can yield up to
        // 30 character entries (name + aliases + role + voice + evidence).
        let tuning = llm::ChatTuning {
            temperature: 0.4,
            num_ctx: 32_768,
            num_predict: 4_096,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.char_stream = Some(handle);
        self.char_extract_error = None;
        // Don't drop existing proposals until we know the new extraction
        // succeeded; user may still be reviewing them.
    }

    pub fn poll_char_extract_stream(&mut self) {
        let Some(stream) = self.char_stream.as_mut() else { return };
        let _ = stream.poll();
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.char_stream = None;
        self.last_char_buffer = Some(buffer.clone());
        if let Some(e) = err {
            log::error!("character extraction stream error: {e}");
            self.char_extract_error = Some(e);
            return;
        }
        let Some(book) = &self.book else { return };
        match llm::characters::parse_characters(&buffer) {
            Some(raw) => {
                let count = raw.characters.len();
                let proposals = llm::characters::diff_against_entities(raw, &book.entities);
                let new_count = proposals
                    .iter()
                    .filter(|p| {
                        matches!(p.verdict, llm::characters::ProposalVerdict::New)
                    })
                    .count();
                let dup_count = proposals.len() - new_count;
                log::info!(
                    "character extraction parsed: raw={count} proposals={} new={new_count} duplicates={dup_count}",
                    proposals.len()
                );
                if proposals.is_empty() {
                    self.char_extract_error = Some(format!(
                        "extraction returned {count} candidate(s) but none survived dedup"
                    ));
                }
                self.char_proposals = proposals;
            }
            None => {
                log::warn!(
                    "character extraction: could not parse JSON ({} bytes returned)",
                    buffer.len()
                );
                self.char_extract_error =
                    Some("could not parse JSON from model response (see log)".into());
            }
        }
    }

    pub fn accept_char_proposal(&mut self, idx: usize) {
        let Some(book) = self.book.as_mut() else { return };
        let Some(p) = self.char_proposals.get_mut(idx) else { return };
        if p.status != ProposalStatus::Pending {
            return;
        }
        let first_seen = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.clone())
            .unwrap_or_default();
        let entity = llm::characters::build_entity(&p.raw, &book.entities, &first_seen);
        let entity_id = entity.id.clone();
        let entity_name = entity.name.clone();
        if let Err(err) = book.save_entity(entity) {
            log::error!(
                "save proposed character {entity_id:?} ({entity_name:?}) failed: {err}"
            );
            self.last_error = Some(format!("save proposed character: {err}"));
            return;
        }
        log::info!(
            "character proposal accepted: id={entity_id:?} name={entity_name:?} first_seen={first_seen:?}"
        );
        p.status = ProposalStatus::Added;
        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
    }

    pub fn dismiss_char_proposal(&mut self, idx: usize) {
        if let Some(p) = self.char_proposals.get_mut(idx) {
            p.status = ProposalStatus::Dismissed;
        }
    }

    /// Accept every Pending proposal whose verdict is `New`. Duplicates and
    /// already-handled rows are left alone.
    pub fn accept_all_new_char_proposals(&mut self) {
        let indices: Vec<usize> = self
            .char_proposals
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.status == ProposalStatus::Pending
                    && matches!(p.verdict, crate::llm::characters::ProposalVerdict::New)
            })
            .map(|(i, _)| i)
            .collect();
        log::info!("accepting {} new character proposals in bulk", indices.len());
        for i in indices {
            self.accept_char_proposal(i);
        }
    }

    pub fn clear_char_proposals(&mut self) {
        self.char_proposals.clear();
        self.char_extract_error = None;
    }

    /// Send the current chat input to the model. Builds a fresh system prompt
    /// with the live chapter prose so the assistant always answers against
    /// what's on screen, not a stale snapshot.
    pub fn send_chat_message(&mut self) {
        if self.chat_stream.is_some() {
            return;
        }
        let text = self.chat_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if !self.ollama_ok {
            self.chat_error = Some("ollama unreachable".into());
            return;
        }
        let Some(book) = self.book.as_ref() else {
            self.chat_error = Some("open a book first".into());
            return;
        };
        let Some(ch) = self.current_chapter.clone() else {
            self.chat_error = Some("open a chapter first".into());
            return;
        };
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.chat_error = Some("the chapter is empty".into());
            return;
        }
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system =
            crate::llm::conversation::build_system(book, &in_scope, &ch.display_title, &prose);

        self.chat_messages.push(llm::ChatMessage::user(text));
        self.chat_input.clear();
        self.chat_error = None;
        self.chat_chapter = Some(ch.file_path.clone());

        let mut messages = Vec::with_capacity(self.chat_messages.len() + 1);
        messages.push(llm::ChatMessage::system(system));
        messages.extend(self.chat_messages.iter().cloned());

        let tuning = llm::ChatTuning {
            temperature: 0.6,
            num_ctx: 32_768,
            num_predict: 1_024,
        };
        log::info!(
            "chat send: chapter={:?} history_turns={} prose_chars={}",
            ch.display_title,
            self.chat_messages.len(),
            prose.chars().count(),
        );
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            false,
            tuning,
        );
        self.chat_stream = Some(handle);
        self.chat_pending_assistant.clear();
    }

    pub fn poll_chat_stream(&mut self) {
        let Some(stream) = self.chat_stream.as_mut() else { return };
        let _ = stream.poll();
        if !stream.buffer.is_empty() {
            self.chat_pending_assistant = stream.buffer.clone();
        }
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.chat_stream = None;
        if let Some(e) = err {
            log::error!("chat stream error: {e}");
            self.chat_error = Some(e);
            self.chat_pending_assistant.clear();
            return;
        }
        let trimmed = buffer.trim();
        if trimmed.is_empty() {
            self.chat_error = Some("model returned an empty response".into());
            self.chat_pending_assistant.clear();
            return;
        }
        self.chat_messages
            .push(llm::ChatMessage::assistant(trimmed.to_string()));
        self.chat_pending_assistant.clear();
    }

    pub fn reset_chat(&mut self) {
        self.chat_messages.clear();
        self.chat_input.clear();
        self.chat_stream = None;
        self.chat_pending_assistant.clear();
        self.chat_error = None;
        self.chat_chapter = None;
    }

    /// Kick off the per-character progression run for the selected character
    /// against the current chapter. Auto-commits the entity dirty buffer first
    /// if it targets the same character — otherwise the AI append would be
    /// clobbered when the user later clicks Save on a stale form.
    pub fn track_progression_for(&mut self, entity_id: &str) {
        if self.progression_stream.is_some() {
            self.progression_status = Some("progression run already in flight".into());
            return;
        }
        if !self.ollama_ok {
            self.progression_status = Some("ollama unreachable".into());
            return;
        }
        let Some(ch) = self.current_chapter.clone() else {
            self.progression_status = Some("open a chapter first".into());
            return;
        };
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.progression_status = Some("nothing to analyse".into());
            return;
        }

        // entity_dirty is always populated while the inspector is open; only
        // auto-commit when the working copy actually differs from disk.
        let needs_commit = match (self.entity_dirty.as_ref(), self.book.as_ref()) {
            (Some(d), Some(book)) if d.id == entity_id => {
                book.entity(entity_id).map(|saved| saved != d).unwrap_or(true)
            }
            _ => false,
        };
        if needs_commit {
            self.commit_entity_edit();
        }

        let Some(book) = self.book.as_ref() else { return };
        let Some(entity) = book.entity(entity_id).cloned() else {
            self.progression_status = Some("character not found".into());
            return;
        };

        let last = entity.progression.last().map(|p| {
            crate::llm::progression::LastSnapshot {
                chapter: p.chapter.clone(),
                voice_summary: p.voice_summary.clone(),
                notable_changes: p.notable_changes.clone(),
            }
        });
        let user_prompt = crate::llm::progression::build_user(
            &entity.name,
            &entity.aliases,
            &entity.voice_notes,
            last.as_ref(),
            &ch.include_path,
            &prose,
        );
        let messages = vec![
            llm::ChatMessage::system(crate::llm::progression::SYSTEM_PROMPT.to_string()),
            llm::ChatMessage::user(user_prompt),
        ];
        // Output is a single small JSON snapshot per character; 1k is plenty.
        let tuning = llm::ChatTuning {
            temperature: 0.4,
            num_ctx: 32_768,
            num_predict: 1_024,
        };
        log::info!(
            "progression run start: entity={entity_id:?} chapter={:?} prose_chars={}",
            ch.include_path,
            prose.chars().count()
        );
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.progression_stream = Some(handle);
        self.progression_target = Some((entity_id.to_string(), ch.include_path.clone()));
        self.progression_status = Some("extracting…".into());
    }

    pub fn poll_progression_stream(&mut self) {
        let Some(stream) = self.progression_stream.as_mut() else { return };
        let _ = stream.poll();
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.progression_stream = None;
        let Some((entity_id, chapter)) = self.progression_target.take() else { return };
        if let Some(e) = err {
            log::error!("progression stream error: {e}");
            self.progression_status = Some(format!("error: {e}"));
            return;
        }
        let Some(raw) = crate::llm::progression::parse(&buffer) else {
            log::warn!(
                "progression: could not parse JSON ({} bytes returned)",
                buffer.len()
            );
            self.progression_status =
                Some("could not parse JSON from model response (see log)".into());
            return;
        };
        if raw.is_empty() {
            log::info!(
                "progression: model returned empty snapshot for {entity_id:?} in {chapter:?} — character likely absent"
            );
            self.progression_status = Some("character not present in this chapter".into());
            return;
        }
        let Some(book) = self.book.as_mut() else { return };
        let Some(mut entity) = book.entity(&entity_id).cloned() else { return };
        entity.progression.push(crate::book::entity::ProgressionEntry {
            chapter: chapter.clone(),
            tone: raw.tone.trim().to_string(),
            situation: raw.situation.trim().to_string(),
            voice_summary: raw.voice_summary.trim().to_string(),
            notable_changes: raw.notable_changes.trim().to_string(),
        });
        if let Err(err) = book.save_entity(entity) {
            log::error!("progression save_entity failed: {err}");
            self.progression_status = Some(format!("save failed: {err}"));
            return;
        }
        log::info!("progression: appended snapshot for {entity_id:?} in {chapter:?}");
        self.progression_status = Some(format!("snapshot saved for {chapter}"));
    }

    pub fn run_pipeline(&mut self, pipeline: Pipeline) {
        let Some(book) = &self.book else { return };
        if self.stream.is_some() {
            return;
        }
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.last_error = Some("nothing to send".into());
            return;
        }
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system = crate::llm::prompts::build_system(book, &in_scope, pipeline);
        let user = crate::llm::prompts::build_user(&prose);

        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "pipeline={} start: chapter={chapter_label:?} prose_chars={} system_bytes={} user_bytes={} model={}",
            pipeline.label(),
            prose.chars().count(),
            system.len(),
            user.len(),
            self.settings.model,
        );

        let messages = vec![
            llm::ChatMessage::system(system),
            llm::ChatMessage::user(user),
        ];
        // Full-chapter prose can run 40-90KB; 32k tokens fits the system
        // prompt + chapter without truncation. Output cap covers up to ~8
        // flags' worth of quote/why/suggestion strings.
        let tuning = llm::ChatTuning {
            temperature: 0.4,
            num_ctx: 32_768,
            num_predict: 2_048,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.stream = Some(handle);
        self.stream_pipeline = Some(pipeline);
        self.last_error = None;
    }

    pub fn poll_stream(&mut self) {
        let Some(stream) = self.stream.as_mut() else { return };
        let _ = stream.poll();
        if stream.done {
            let buffer = std::mem::take(&mut stream.buffer);
            let pipeline = self.stream_pipeline.take().unwrap_or(Pipeline::Voice);
            let was_repair = std::mem::take(&mut self.stream_is_repair);
            let err = stream.error.take();
            self.stream = None;
            self.last_stream_buffer = Some(buffer.clone());
            if let Some(e) = err {
                self.last_error = Some(e);
                return;
            }
            let outcome = self.ingest_response(pipeline, &buffer);
            if outcome == IngestOutcome::NeedsRepair {
                // Dump the unsalvageable response so we can study the patterns
                // offline and tighten the salvage parser. Failures from the
                // first attempt and the repair attempt land in the same dir
                // with distinct suffixes.
                let suffix = if was_repair { "broken-after-repair" } else { "broken" };
                dump_unsalvageable(pipeline, &buffer, suffix);
                if !was_repair {
                    // One last attempt: hand the broken text back to the model
                    // and ask it to repair the JSON. Guarded by `was_repair`
                    // so a malformed repair response can't trigger another.
                    self.start_json_repair(pipeline, &buffer);
                }
            }
        }
    }

    fn start_json_repair(&mut self, pipeline: Pipeline, broken: &str) {
        log::warn!(
            "pipeline={} repair: asking model to fix malformed JSON ({} bytes)",
            pipeline.label(),
            broken.len(),
        );
        let system = "You are a JSON repair tool. The input is a JSON document that failed to parse. \
Output only the corrected JSON — no commentary, no code fences, no explanation. \
Preserve every valid array element; drop or fix invalid tokens (stray identifiers, \
missing commas, unbalanced braces) to produce strict, parseable JSON. \
The target shape is `{\"flags\":[{\"kind\":\"...\",\"quote\":\"...\",\"why\":\"...\",\"suggestion\":\"...\"}]}`.";
        let user = format!("Fix this JSON:\n\n{broken}");
        let messages = vec![
            llm::ChatMessage::system(system.to_string()),
            llm::ChatMessage::user(user),
        ];
        // Repair runs on a much smaller payload than the original prompt
        // (just the broken response + a short instruction). Same num_predict
        // ceiling so we don't truncate the corrected output.
        let tuning = llm::ChatTuning {
            temperature: 0.1,
            num_ctx: 16_384,
            num_predict: 2_048,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.stream = Some(handle);
        self.stream_pipeline = Some(pipeline);
        self.stream_is_repair = true;
        self.last_error = Some(format!(
            "{}: response was malformed; asking model to repair…",
            pipeline.label()
        ));
    }

    fn ingest_response(&mut self, pipeline: Pipeline, buffer: &str) -> IngestOutcome {
        use crate::llm::revision::{anchor, parse_flags_only, parse_voice, FlagKind};
        let prose = latex::to_prose(&self.editor_text);
        let parsed_ok;
        let flags = match pipeline {
            Pipeline::Voice => match parse_voice(buffer) {
                Some(v) => {
                    parsed_ok = true;
                    v.flags
                }
                None => {
                    parsed_ok = false;
                    Vec::new()
                }
            },
            Pipeline::ShowDontTell | Pipeline::Prose | Pipeline::Spelling => {
                match parse_flags_only(buffer) {
                    Some(v) => {
                        parsed_ok = true;
                        v.flags
                    }
                    None => {
                        parsed_ok = false;
                        Vec::new()
                    }
                }
            }
        };
        if !parsed_ok {
            log::warn!(
                "pipeline={} parse failed: response_bytes={} preview={:?}",
                pipeline.label(),
                buffer.len(),
                preview_for_log(buffer, 240),
            );
        }

        let raw_count = flags.len();
        let mut added = 0usize;
        let mut anchored = 0usize;
        for f in flags {
            if f.quote.trim().is_empty() {
                continue;
            }
            // Try anchoring against the original LaTeX text first; fall back to
            // anchor in the prose-stripped string and translate by string search.
            let anchor_in_text = anchor(&self.editor_text, &f.quote)
                .or_else(|| anchor(&prose, &f.quote).and_then(|_| anchor(&self.editor_text, f.quote.trim())));
            if anchor_in_text.is_some() {
                anchored += 1;
            }

            // Only the spelling pipeline ships per-flag categories;
            // voice/show/prose collapse to FlagKind::Other and are coloured
            // by their pipeline instead.
            let kind = if pipeline == Pipeline::Spelling {
                FlagKind::parse(&f.kind)
            } else {
                FlagKind::Other
            };

            let id = self.next_rev_id;
            self.next_rev_id += 1;
            self.revisions.push(Revision {
                id,
                pipeline,
                kind,
                quote: f.quote.clone(),
                why: f.why.clone(),
                suggestion: f.suggestion.clone(),
                anchor: anchor_in_text,
            });
            added += 1;
        }
        // Sort by position in the source so the cards read in reading order;
        // unanchored flags sink to the bottom. Stable sort preserves id order
        // among same-position ties.
        self.revisions
            .sort_by_key(|r| r.anchor.map(|(s, _)| s).unwrap_or(usize::MAX));
        log::info!(
            "pipeline={} ingested: parsed_ok={parsed_ok} raw_flags={raw_count} added={added} anchored={anchored} response_bytes={}",
            pipeline.label(),
            buffer.len(),
        );
        if added == 0 {
            let msg = if !parsed_ok {
                format!(
                    "{}: JSON parse failed — model likely returned prose, not JSON (see log)",
                    pipeline.label()
                )
            } else {
                format!("{}: model returned 0 flags", pipeline.label())
            };
            self.last_error = Some(msg);
        }
        if parsed_ok {
            IngestOutcome::Done
        } else {
            IngestOutcome::NeedsRepair
        }
    }

    pub fn accept_revision(&mut self, id: u32) {
        let Some(idx) = self.revisions.iter().position(|r| r.id == id) else { return };
        let rev = self.revisions[idx].clone();
        let Some((s, e)) = rev.anchor else { return };
        if e > self.editor_text.len() || s > e {
            return;
        }
        if self.selected_revision == Some(id) {
            self.selected_revision = None;
        }
        self.editor_text.replace_range(s..e, &rev.suggestion);
        self.dirty = true;
        // Drop the accepted revision and shift remaining anchors past the
        // replacement point. Overlapping anchors lose their position because
        // the underlying span no longer exists.
        let delta = rev.suggestion.len() as isize - (e - s) as isize;
        self.revisions.remove(idx);
        for r in &mut self.revisions {
            if let Some((rs, re)) = r.anchor {
                if rs >= e {
                    let new_s = (rs as isize + delta).max(0) as usize;
                    let new_e = (re as isize + delta).max(0) as usize;
                    r.anchor = Some((new_s, new_e));
                } else if re > s {
                    r.anchor = None;
                }
            }
        }
        self.refresh_entity_hits();
    }

    pub fn dismiss_revision(&mut self, id: u32) {
        self.revisions.retain(|r| r.id != id);
        if self.selected_revision == Some(id) {
            self.selected_revision = None;
        }
    }

    /// Select a revision card and jump the editor to its anchor. Toggling the
    /// same revision off returns nothing to the editor (cursor stays put).
    pub fn select_revision(&mut self, id: u32) {
        if self.selected_revision == Some(id) {
            log::info!("select_revision: deselecting id={id}");
            self.selected_revision = None;
            return;
        }
        let anchor = self
            .revisions
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.anchor);
        log::info!("select_revision: id={id} anchor={anchor:?}");
        self.selected_revision = Some(id);
        if let Some((s, _)) = anchor {
            self.jump_to_anchor(s);
        }
    }

    fn load_notes(&mut self) {
        let Some(ch) = &self.current_chapter else { return };
        let p = ch.file_path.with_extension("notes.md");
        self.notes_text = std::fs::read_to_string(&p).unwrap_or_default();
        self.notes_dirty = false;
        self.notes_path = Some(p);
    }

    pub fn start_pdf_build(&mut self) {
        let Some(book) = &self.book else { return };
        if self.pdf_building {
            return;
        }
        self.pdf_error = None;
        self.pdf_building = true;
        self.pdf_meta = None;
        self.pdf_renderer = None;
        self.pdf_textures.clear();
        self.pdf_build_rx = Some(pdf::build_and_meta(&book.root, self.pdf_dpi));
    }

    /// Open Read mode against an already-built PDF: just read metadata, don't
    /// rasterize anything up front. Pages render on demand from `pdf_view`.
    pub fn open_existing_pdf(&mut self) {
        let Some(book) = &self.book else { return };
        if self.pdf_building {
            return;
        }
        self.pdf_error = None;
        self.pdf_building = true;
        self.pdf_meta = None;
        self.pdf_renderer = None;
        self.pdf_textures.clear();
        self.pdf_build_rx = Some(pdf::meta_only(&book.root, self.pdf_dpi));
    }

    fn poll_pdf_build(&mut self) {
        let Some(rx) = &self.pdf_build_rx else { return };
        match rx.try_recv() {
            Ok(pdf::BuildOutcome::Built(meta)) => {
                let book_root = self.book.as_ref().map(|b| b.root.clone());
                self.pdf_renderer = book_root.map(|root| {
                    pdf::PageRenderer::new(&root, meta.dpi, meta.page_count)
                });
                self.pdf_meta = Some(meta);
                self.pdf_textures.clear();
                self.pdf_building = false;
                self.pdf_build_rx = None;
                self.pdf_error = None;
            }
            Ok(pdf::BuildOutcome::Failed(msg)) => {
                self.pdf_error = Some(msg);
                self.pdf_building = false;
                self.pdf_build_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pdf_building = false;
                self.pdf_build_rx = None;
            }
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

    pub fn save_notes(&mut self) {
        let Some(p) = self.notes_path.clone() else { return };
        if let Err(e) = std::fs::write(&p, &self.notes_text) {
            self.last_error = Some(format!("notes save: {e}"));
            return;
        }
        self.notes_dirty = false;
    }

    fn check_ollama(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.last_ollama_check < 5.0 {
            return;
        }
        self.last_ollama_check = now;
        match llm::ping(&self.settings.ollama_url) {
            Ok(tags) => {
                self.ollama_ok = !tags.is_empty();
                self.available_models = tags;
            }
            Err(_) => {
                self.ollama_ok = false;
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let save = ctx.input(|i| {
            i.modifiers.command && i.key_pressed(egui::Key::S)
        });
        if save && self.dirty {
            if let Err(e) = self.save_chapter() {
                self.last_error = Some(format!("save: {e}"));
            }
        }
        if self.notes_dirty
            && ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S))
        {
            self.save_notes();
        }
    }

    fn book_picker(&mut self, ctx: &egui::Context) {
        if !self.show_book_picker {
            return;
        }
        let mut open_now: Option<PathBuf> = None;
        let mut close = false;

        egui::Window::new("Open book")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Path to book root (a directory containing main.tex):");
                ui.add(egui::TextEdit::singleline(&mut self.picker_path).desired_width(420.0));
                ui.horizontal(|ui| {
                    if ui.button("Open").clicked() {
                        open_now = Some(PathBuf::from(self.picker_path.trim()));
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                if !self.settings.recent_books.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("recent").small().color(theme::TEXT_MUTED));
                    let recents = self.settings.recent_books.clone();
                    for r in recents {
                        if ui.link(r.display().to_string()).clicked() {
                            open_now = Some(r);
                        }
                    }
                }
            });

        if let Some(p) = open_now {
            if let Err(e) = self.open_book(&p) {
                self.last_error = Some(format!("open book: {e}"));
            } else {
                self.show_book_picker = false;
            }
        }
        if close {
            self.show_book_picker = false;
        }
    }
}

fn apply_mirror_op(book: &mut Book, self_id: &str, op: MirrorOp) -> anyhow::Result<()> {
    match op {
        MirrorOp::Add { target, kind } => {
            let Some(mut t) = book.entity(&target).cloned() else {
                log::debug!("mirror Add skipped: target {target:?} not found");
                return Ok(());
            };
            // Idempotent: don't double-add the same (kind, self_id).
            let exists = t.relations.iter().any(|r| {
                r.kind.eq_ignore_ascii_case(&kind) && r.id.eq_ignore_ascii_case(self_id)
            });
            if exists {
                return Ok(());
            }
            t.relations.push(Relation {
                kind,
                id: self_id.to_string(),
            });
            book.save_entity(t)?;
        }
        MirrorOp::Remove { target, kind } => {
            let Some(mut t) = book.entity(&target).cloned() else {
                return Ok(());
            };
            let before = t.relations.len();
            t.relations.retain(|r| {
                !(r.kind.eq_ignore_ascii_case(&kind) && r.id.eq_ignore_ascii_case(self_id))
            });
            if t.relations.len() != before {
                book.save_entity(t)?;
            }
        }
    }
    Ok(())
}

/// Persist a malformed LLM response to `<repo>/test-results/` so we can study
/// the failure pattern offline and harden the salvage parser. Uses the build
/// directory of the binary so dumps land in the source tree, not the user's
/// state dir. Failures here are non-fatal — we just log and move on.
fn dump_unsalvageable(pipeline: Pipeline, buffer: &str, suffix: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-results");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("test-results: cannot create {}: {e}", dir.display());
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{ts}-{}-{suffix}.json", pipeline.label()));
    if let Err(e) = std::fs::write(&path, buffer) {
        log::warn!("test-results: write failed for {}: {e}", path.display());
        return;
    }
    log::info!(
        "test-results: dumped malformed {} response ({} bytes) to {}",
        pipeline.label(),
        buffer.len(),
        path.display()
    );
}

fn preview_for_log(s: &str, max: usize) -> String {
    let escaped: String = s
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    if escaped.chars().count() <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn kind_singular_id(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Character => "character",
        EntityKind::Location => "location",
        EntityKind::Event => "event",
        EntityKind::Timeline => "timeline",
    }
}

impl App for CkWriterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_ollama(ctx);
        self.handle_shortcuts(ctx);
        self.poll_stream();
        self.poll_char_extract_stream();
        self.poll_progression_stream();
        self.poll_chat_stream();
        self.poll_pdf_build();
        if let Some(r) = self.pdf_renderer.as_mut() {
            if r.poll() {
                ctx.request_repaint();
            }
            if r.has_inflight() {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        if self.pdf_building {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        // Repaint while a stream is running so tokens show up promptly.
        if self.stream.is_some()
            || self.char_stream.is_some()
            || self.progression_stream.is_some()
            || self.chat_stream.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| ui::top_bar::show(self, ui));

        let left_resp = egui::SidePanel::left("chapters")
            .default_width(self.settings.left_panel_width)
            .resizable(true)
            .show(ctx, |ui| {
                ui::file_tree::show(self, ui);
            });
        let lw = left_resp.response.rect.width();
        if (lw - self.settings.left_panel_width).abs() > 1.0 {
            self.settings.left_panel_width = lw;
            self.settings_dirty = true;
        }

        // Defensive clamp: an earlier bug let the panel widen onto the editor
        // area; re-opening the app would honour that bad width otherwise.
        let saved_rw = self.settings.right_panel_width.clamp(280.0, 1200.0);
        // Cast / Personae render their own master/detail inline, so the bottom
        // inspector would just duplicate the same form.
        let inline_detail_active = self.scope_tab == ui::scope_panel::Tab::Characters
            && matches!(
                self.char_sub_tab,
                ui::scope_panel::CharSubTab::Cast | ui::scope_panel::CharSubTab::Personae
            );
        // Entity details only belong on tabs that are about entities — keeping
        // them visible on AI / Chat / Notes leaks an unrelated character form
        // under the panel.
        let inspector_relevant = matches!(
            self.scope_tab,
            ui::scope_panel::Tab::Characters | ui::scope_panel::Tab::Locations
        );
        let bottom_inspector =
            self.selected_entity.is_some() && !inline_detail_active && inspector_relevant;
        let right_resp = egui::SidePanel::right("scope")
            .default_width(saved_rw)
            .resizable(true)
            .show(ctx, |ui| {
                if bottom_inspector {
                    let total_h = ui.available_height();
                    let scope_h = (total_h * 0.50).clamp(180.0, (total_h - 200.0).max(180.0));
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), scope_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui::scope_panel::show(self, ui);
                        },
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("inspector-scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui::inspector::show(self, ui);
                        });
                } else {
                    ui::scope_panel::show(self, ui);
                }
            });
        let rw = right_resp.response.rect.width();
        if (rw - self.settings.right_panel_width).abs() > 1.0 {
            self.settings.right_panel_width = rw;
            self.settings_dirty = true;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.read_mode {
                ui::pdf_view::show(self, ui);
            } else {
                ui::editor::show(self, ui);
            }
        });

        self.book_picker(ctx);
        ui::settings_dialog::show(self, ctx);

        if !self.show_book_picker && self.book.is_none() {
            // Welcome state: nudge the picker open.
            self.show_book_picker = true;
        }

        if self.settings_dirty {
            let now = ctx.input(|i| i.time);
            if now - self.last_settings_save > 1.0 {
                let _ = self.settings.save();
                self.settings_dirty = false;
                self.last_settings_save = now;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(1100));
            }
        }
    }
}
