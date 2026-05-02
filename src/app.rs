use crate::book::entity::{slugify, Entity, EntityKind};
use crate::book::{latex, Book, Chapter};
use crate::extract::{EntityHit, EntityMatcher};
use crate::llm;
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{Revision, RevisionStatus};
use crate::pdf;
use crate::scope;
use crate::settings::Settings;
use crate::theme;
use crate::ui;
use eframe::App;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub struct CkWriterApp {
    pub settings: Settings,
    pub book: Option<Book>,
    pub matcher: Option<EntityMatcher>,
    pub current_chapter: Option<Chapter>,
    pub editor_text: String,
    pub dirty: bool,
    pub entity_hits: Vec<EntityHit>,

    pub scope_tab: ui::scope_panel::Tab,
    pub selected_entity: Option<String>,
    pub entity_dirty: Option<Entity>,

    pub stream: Option<llm::StreamHandle>,
    pub stream_pipeline: Option<Pipeline>,
    pub last_stream_buffer: Option<String>,
    pub revisions: Vec<Revision>,
    pub next_rev_id: u32,
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
            selected_entity: None,
            entity_dirty: None,
            stream: None,
            stream_pipeline: None,
            last_stream_buffer: None,
            revisions: Vec::new(),
            next_rev_id: 1,
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
        self.dirty = false;
        self.selected_entity = None;
        self.entity_dirty = None;
        self.notes_text.clear();
        self.notes_dirty = false;
        self.notes_path = None;
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
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.editor_text = text;
                self.dirty = false;
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
        if let Err(err) = book.save_entity(e) {
            self.last_error = Some(format!("save entity: {err}"));
            return;
        }
        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
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

        let messages = vec![
            llm::ChatMessage::system(system),
            llm::ChatMessage::user(user),
        ];
        let handle = llm::chat_stream(&self.settings.ollama_url, &self.settings.model, messages, true);
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
            let err = stream.error.take();
            self.stream = None;
            self.last_stream_buffer = Some(buffer.clone());
            if let Some(e) = err {
                self.last_error = Some(e);
                return;
            }
            self.ingest_response(pipeline, &buffer);
        }
    }

    fn ingest_response(&mut self, pipeline: Pipeline, buffer: &str) {
        use crate::llm::revision::{anchor, parse_flags_only, parse_voice};
        let prose = latex::to_prose(&self.editor_text);
        let flags = match pipeline {
            Pipeline::Voice => parse_voice(buffer)
                .map(|v| v.flags)
                .unwrap_or_default(),
            Pipeline::ShowDontTell | Pipeline::Prose => parse_flags_only(buffer)
                .map(|v| v.flags)
                .unwrap_or_default(),
        };

        let mut added = 0usize;
        for f in flags {
            if f.quote.trim().is_empty() {
                continue;
            }
            // Try anchoring against the original LaTeX text first; fall back to
            // anchor in the prose-stripped string and translate by string search.
            let anchor_in_text = anchor(&self.editor_text, &f.quote)
                .or_else(|| anchor(&prose, &f.quote).and_then(|_| anchor(&self.editor_text, f.quote.trim())));

            let id = self.next_rev_id;
            self.next_rev_id += 1;
            self.revisions.push(Revision {
                id,
                pipeline,
                quote: f.quote.clone(),
                why: f.why.clone(),
                suggestion: f.suggestion.clone(),
                anchor: anchor_in_text,
                status: RevisionStatus::Pending,
            });
            added += 1;
        }
        if added == 0 {
            self.last_error = Some(format!("{}: no flags returned (or JSON parse failed)", pipeline.label()));
        }
    }

    pub fn accept_revision(&mut self, id: u32) {
        let Some(idx) = self.revisions.iter().position(|r| r.id == id) else { return };
        let rev = self.revisions[idx].clone();
        let Some((s, e)) = rev.anchor else { return };
        if e > self.editor_text.len() || s > e {
            return;
        }
        self.editor_text.replace_range(s..e, &rev.suggestion);
        self.dirty = true;
        // Shift other anchors that come after this point.
        let delta = rev.suggestion.len() as isize - (e - s) as isize;
        for r in &mut self.revisions {
            if r.id == id {
                r.status = RevisionStatus::Accepted;
                r.anchor = None;
                continue;
            }
            if let Some((rs, re)) = r.anchor {
                if rs >= e {
                    let new_s = (rs as isize + delta).max(0) as usize;
                    let new_e = (re as isize + delta).max(0) as usize;
                    r.anchor = Some((new_s, new_e));
                } else if re > s {
                    r.anchor = None; // overlaps replaced region
                }
            }
        }
        self.refresh_entity_hits();
    }

    pub fn dismiss_revision(&mut self, id: u32) {
        if let Some(r) = self.revisions.iter_mut().find(|r| r.id == id) {
            r.status = RevisionStatus::Dismissed;
            r.anchor = None;
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
        if self.stream.is_some() {
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

        let right_resp = egui::SidePanel::right("scope")
            .default_width(self.settings.right_panel_width)
            .resizable(true)
            .show(ctx, |ui| {
                ui::scope_panel::show(self, ui);
                ui.separator();
                ui::inspector::show(self, ui);
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
