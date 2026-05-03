use crate::book::entity::Entity;
use crate::book::paragraphs::Paragraph;
use crate::book::{Book, Chapter};
use crate::extract::{EntityHit, EntityMatcher};
use crate::index::CrossChapterIndex;
use crate::llm;
use crate::llm::characters::ProposedCharacter;
use crate::llm::prompts::Pipeline;
use crate::llm::revision::Revision;
use crate::settings::Settings;
use crate::theme;
use crate::ui;
use eframe::App;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

/// In-progress state for the "+ New chapter" modal. Folder defaults to the
/// first managed folder (`Ancient`) so the writer can usually just type a
/// title and hit Enter.
#[derive(Debug, Clone)]
pub struct NewChapterDialog {
    pub folder: String,
    pub title: String,
}

impl Default for NewChapterDialog {
    fn default() -> Self {
        Self {
            folder: crate::book::manuscript::MANAGED_FOLDERS
                .first()
                .map(|s| (*s).to_string())
                .unwrap_or_default(),
            title: String::new(),
        }
    }
}

mod book;
mod chat;
mod coach;
mod entity;
mod extract;
mod pdf;
mod progression;

/// Editable view of the current chapter's metadata, bound to the right-panel
/// Chapter tab. Only the writer-editable fields live here; read-only fields
/// (word_count, voice_score, last_coached_at) are read straight off the
/// chapter's saved meta on render.
#[derive(Debug, Clone, Default)]
pub struct ChapterDraft {
    /// Stable folder/name pair so we know which chapter the buffer belongs
    /// to. If the user switches chapters before saving, the dirty draft is
    /// dropped and re-seeded for the new chapter.
    pub folder: String,
    pub name: String,
    pub summary: String,
    pub goals: String,
    pub plot_notes: String,
    pub dirty: bool,
}

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

    // === Book / chapter / editor state ===
    pub book: Option<Book>,
    pub matcher: Option<EntityMatcher>,
    pub current_chapter: Option<Chapter>,
    pub editor_text: String,
    pub dirty: bool,
    pub entity_hits: Vec<EntityHit>,
    /// Paragraph index for the open chapter, in source order. Recomputed on
    /// chapter open and on save; cleared when no chapter is open. Drives
    /// per-paragraph caching (#0004) and cursor-to-paragraph mapping (#0005).
    pub current_paragraphs: Vec<Paragraph>,

    // === Entity editing ===
    pub scope_tab: ui::scope_panel::Tab,
    pub char_sub_tab: ui::scope_panel::CharSubTab,
    pub selected_entity: Option<String>,
    pub entity_dirty: Option<Entity>,
    /// Search filter shared by Cast / Personae master lists.
    pub character_search: String,

    // === Coach pipeline (voice/show/prose/spelling) ===
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

    // === Character extraction ===
    /// Separate stream so character extraction never collides with the
    /// voice/show/prose coaching pipelines.
    pub char_stream: Option<llm::StreamHandle>,
    pub last_char_buffer: Option<String>,
    pub char_proposals: Vec<ProposedCharacter>,
    pub char_extract_error: Option<String>,

    // === Progression ===
    /// Per-character progression run. Independent of char/coach streams so
    /// the writer can fire one without blocking other AI work.
    pub progression_stream: Option<llm::StreamHandle>,
    /// (entity_id, chapter_include_path) — what the in-flight stream is for.
    pub progression_target: Option<(String, String)>,
    pub progression_status: Option<String>,
    /// Cross-chapter occurrence index. Rebuilt lazily after entity edits or
    /// chapter saves so the inspector can show "Appears in N chapters".
    pub char_index: Option<CrossChapterIndex>,

    // === Chat ===
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

    // === Ollama health ===
    pub last_error: Option<String>,
    pub ollama_ok: bool,
    pub last_ollama_check: f64,
    pub available_models: Vec<String>,

    // === Book picker / settings dialog / import ===
    pub show_book_picker: bool,
    pub picker_path: String,
    /// Directory paths currently expanded in the file tree sidebar.
    pub expanded_dirs: HashSet<PathBuf>,
    /// Active tab in the left sidebar (Manuscript vs All Files).
    pub file_tree_tab: ui::file_tree::FileTreeTab,
    pub show_settings: bool,
    pub settings_dirty: bool,
    pub last_settings_save: f64,
    pub import_status: Option<String>,

    // === Chapter-management dialogs ===
    /// When `Some`, the "+ New chapter" modal is open. The fields hold the
    /// in-progress folder selection and title input; cleared when the user
    /// confirms or cancels.
    pub new_chapter_dialog: Option<NewChapterDialog>,
    /// When `Some`, a "Delete chapter?" confirm is open for the named
    /// chapter. The user has to click Delete a second time inside the modal,
    /// since deleting a chapter is unrecoverable from inside CkWriter.
    pub delete_chapter_confirm: Option<(String, String)>,
    /// Last error from a chapter operation, surfaced on the sidebar so
    /// failures don't get lost in the log.
    pub chapter_op_error: Option<String>,

    // === Chapter tab (right panel) ===
    /// Editable buffer for the current chapter's metadata. `None` when no
    /// chapter is open. Re-seeded from `Book::chapter.meta` whenever the
    /// current chapter changes.
    pub chapter_draft: Option<ChapterDraft>,

    // === PDF / read mode ===
    pub read_mode: bool,
    pub pdf_building: bool,
    pub pdf_build_rx: Option<mpsc::Receiver<crate::pdf::BuildOutcome>>,
    pub pdf_meta: Option<crate::pdf::PdfMeta>,
    pub pdf_renderer: Option<crate::pdf::PageRenderer>,
    pub pdf_textures: HashMap<u32, egui::TextureHandle>,
    pub pdf_error: Option<String>,
    pub pdf_dpi: u32,

    // === Editor scroll/cursor ===
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

    // === Diff view ===
    /// When true, the central panel renders the diff view (HEAD baseline on
    /// the left, editable buffer on the right) instead of the plain editor.
    /// Mutually exclusive with `read_mode` — toggling either off the other.
    pub diff_mode: bool,
    /// Cached `git show HEAD:<path>` content for the chapter named in
    /// `diff_baseline_chapter`. Re-loaded when the chapter changes.
    pub diff_baseline: Option<String>,
    pub diff_baseline_chapter: Option<PathBuf>,
    /// Set when baseline lookup ran but produced no usable text (untracked
    /// file, no repo, git error). The view shows this string instead of a
    /// diff so the user understands why nothing rendered.
    pub diff_baseline_error: Option<String>,
    /// Shared vertical scroll offset for the side-by-side diff. Both columns
    /// render at this offset and write their post-scroll state back, so a
    /// wheel turn in either column moves both in lockstep.
    pub diff_scroll_y: f32,
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
            current_paragraphs: Vec::new(),
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
            file_tree_tab: ui::file_tree::FileTreeTab::default(),
            show_settings: false,
            settings_dirty: false,
            last_settings_save: 0.0,
            import_status: None,
            new_chapter_dialog: None,
            delete_chapter_confirm: None,
            chapter_op_error: None,
            chapter_draft: None,
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
            diff_mode: false,
            diff_baseline: None,
            diff_baseline_chapter: None,
            diff_baseline_error: None,
            diff_scroll_y: 0.0,
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
        let save = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S));
        if save && self.dirty {
            if let Err(e) = self.save_chapter() {
                self.last_error = Some(format!("save: {e}"));
            }
        }
        let chapter_dirty = self.chapter_draft.as_ref().is_some_and(|d| d.dirty);
        if chapter_dirty
            && ctx
                .input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S))
        {
            self.save_chapter_draft();
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
                    ui.label(
                        egui::RichText::new("recent")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
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

    fn new_chapter_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.new_chapter_dialog.take() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;

        egui::Window::new("New chapter")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Folder:");
                egui::ComboBox::from_id_salt("new-chapter-folder")
                    .selected_text(&dlg.folder)
                    .show_ui(ui, |ui| {
                        for f in crate::book::manuscript::MANAGED_FOLDERS {
                            ui.selectable_value(&mut dlg.folder, (*f).to_string(), *f);
                        }
                    });
                ui.label("Title:");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut dlg.title)
                        .desired_width(320.0)
                        .hint_text("e.g. First Encounter"),
                );
                resp.request_focus();
                let enter_pressed =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let create_clicked = ui
                        .add_enabled(!dlg.title.trim().is_empty(), egui::Button::new("Create"))
                        .clicked();
                    if create_clicked || enter_pressed {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if confirm {
            let folder = dlg.folder.clone();
            let title = dlg.title.clone();
            match self.add_chapter(&folder, &title) {
                Ok(()) => {
                    self.chapter_op_error = None;
                    self.new_chapter_dialog = None;
                }
                Err(e) => {
                    self.chapter_op_error = Some(format!("add chapter: {e}"));
                    self.new_chapter_dialog = Some(dlg);
                }
            }
        } else if cancel {
            self.new_chapter_dialog = None;
        } else {
            self.new_chapter_dialog = Some(dlg);
        }
    }

    fn delete_chapter_confirm(&mut self, ctx: &egui::Context) {
        let Some((folder, name)) = self.delete_chapter_confirm.clone() else {
            return;
        };
        let mut confirmed = false;
        let mut cancel = false;

        egui::Window::new("Delete chapter?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Permanently delete {folder}/{name}?"));
                ui.label(
                    egui::RichText::new("This removes the .tex file from disk.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if confirmed {
            if let Err(e) = self.delete_chapter(&folder, &name) {
                self.chapter_op_error = Some(format!("delete chapter: {e}"));
            } else {
                self.chapter_op_error = None;
            }
            self.delete_chapter_confirm = None;
        } else if cancel {
            self.delete_chapter_confirm = None;
        }
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
        // them visible on AI / Chat / Chapter leaks an unrelated character
        // form under the panel.
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
            } else if self.diff_mode {
                ui::diff_view::show(self, ui);
            } else {
                ui::editor::show(self, ui);
            }
        });

        self.book_picker(ctx);
        self.new_chapter_dialog(ctx);
        self.delete_chapter_confirm(ctx);
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
