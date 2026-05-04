use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChapterPlace {
    /// Char-index of the cursor (egui TextEdit uses char positions, not bytes).
    #[serde(default)]
    pub cursor: usize,
    /// Vertical scroll offset of the editor's ScrollArea, in pixels.
    #[serde(default)]
    pub scroll: f32,
}

/// Reading-surface font choice (#0020). Drives both the editor and the chat
/// panel — every reading surface in the app shares one selection. Atkinson is
/// the dyslexia-friendly default; iA Writer Quattro is preserved as an option
/// for direct comparison.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingFont {
    #[default]
    AtkinsonHyperlegible,
    OpenDyslexic,
    IaWriterQuattro,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    #[serde(default)]
    pub recent_books: Vec<PathBuf>,
    /// Font for every reading surface (editor + chat). Default is Atkinson
    /// Hyperlegible — the user is dyslexic and the dyslexia-friendly default
    /// is the whole point of the reading-surface knobs (#0020).
    #[serde(default)]
    pub reading_font: ReadingFont,
    /// Body font size in pixels for the editor and chat. Renamed from
    /// `editor_font_size` (#0020) — chat is also a reading surface, so the
    /// knob is shared. Old `settings.toml` files with `editor_font_size = N`
    /// continue to load via the serde alias below.
    #[serde(
        default = "default_reading_font_size",
        alias = "editor_font_size"
    )]
    pub reading_font_size: f32,
    /// Multiplier applied to font size to derive line height. Replaces the
    /// `LINE_HEIGHT_MULTIPLIER` const in `src/ui/editor.rs` (#0020).
    #[serde(default = "default_reading_line_height_mult")]
    pub reading_line_height_mult: f32,
    /// Extra letter spacing in pixels passed into egui's `TextFormat`.
    /// Replaces the hardcoded 0.1 literal in `build_job` (#0020). Default
    /// 0.4 is a moderate bump on the previous 0.1.
    #[serde(default = "default_reading_letter_spacing")]
    pub reading_letter_spacing: f32,
    #[serde(default = "default_left_panel_width")]
    pub left_panel_width: f32,
    #[serde(default = "default_right_panel_width")]
    pub right_panel_width: f32,
    #[serde(default)]
    pub last_book: Option<PathBuf>,
    #[serde(default)]
    pub last_chapter: Option<PathBuf>,
    /// For each book root, the directory paths the user had expanded last time.
    #[serde(default)]
    pub expanded_dirs: HashMap<PathBuf, Vec<PathBuf>>,
    /// For each chapter file, where the cursor was and how far the editor was scrolled.
    #[serde(default)]
    pub chapter_places: HashMap<PathBuf, ChapterPlace>,
    /// Sampling temperature for the coaching pipelines (voice/show/prose/spelling).
    /// Lower values reduce invented flags; the slider snaps to 0.1 increments.
    #[serde(default = "default_coach_temperature")]
    pub coach_temperature: f32,
    /// When true, post-filter coach responses against the per-chapter dismissals
    /// list so previously rejected quotes don't reappear. The model still sees
    /// and "thinks about" those passages — this is a presentation filter only.
    #[serde(default = "default_coach_filter_dismissed")]
    pub coach_filter_dismissed: bool,
}

fn default_model() -> String {
    "gemma4:latest".into()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}
fn default_reading_font_size() -> f32 {
    18.0
}
fn default_reading_line_height_mult() -> f32 {
    1.7
}
fn default_reading_letter_spacing() -> f32 {
    0.4
}
fn default_left_panel_width() -> f32 {
    260.0
}
fn default_right_panel_width() -> f32 {
    600.0
}
fn default_coach_temperature() -> f32 {
    0.2
}
fn default_coach_filter_dismissed() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model(),
            ollama_url: default_ollama_url(),
            recent_books: Vec::new(),
            reading_font: ReadingFont::default(),
            reading_font_size: default_reading_font_size(),
            reading_line_height_mult: default_reading_line_height_mult(),
            reading_letter_spacing: default_reading_letter_spacing(),
            left_panel_width: default_left_panel_width(),
            right_panel_width: default_right_panel_width(),
            last_book: None,
            last_chapter: None,
            expanded_dirs: HashMap::new(),
            chapter_places: HashMap::new(),
            coach_temperature: default_coach_temperature(),
            coach_filter_dismissed: default_coach_filter_dismissed(),
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ckwriter")
        .join("settings.toml")
}

impl Settings {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                log::warn!("settings parse failed ({e}); using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(self)?;
        std::fs::write(&path, s)?;
        Ok(())
    }

    pub fn touch_recent(&mut self, book_root: &Path) {
        let p = book_root.to_path_buf();
        self.recent_books.retain(|x| x != &p);
        self.recent_books.insert(0, p.clone());
        self.recent_books.truncate(10);
        self.last_book = Some(p);
    }
}
