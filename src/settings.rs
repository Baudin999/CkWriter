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

/// App-wide font choice (#0020). Drives every proportional text surface
/// (editor, chat, lists, forms, dialogs). Atkinson is the dyslexia-friendly
/// default; iA Writer Quattro is preserved as an option for direct comparison.
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
    /// App-wide font for every proportional text surface (#0020). The user is
    /// dyslexic and the whole app is a reading surface — chrome, forms, and
    /// dialogs all inherit this through `egui::Style::text_styles`.
    #[serde(default)]
    pub reading_font: ReadingFont,
    /// Body / button font size in pixels (#0020 pivot). Renamed from
    /// `reading_font_size` (which was renamed from `editor_font_size`); both
    /// old keys keep loading via the aliases below.
    #[serde(
        default = "default_font_size_normal",
        alias = "reading_font_size",
        alias = "editor_font_size"
    )]
    pub font_size_normal: f32,
    /// Heading font size in pixels (#0020 pivot). Maps to egui's
    /// `TextStyle::Heading`.
    #[serde(default = "default_font_size_header")]
    pub font_size_header: f32,
    /// Muted / `.small()` font size in pixels (#0020 pivot). Maps to egui's
    /// `TextStyle::Small`.
    #[serde(default = "default_font_size_info")]
    pub font_size_info: f32,
    /// Multiplier applied to `font_size_normal` to derive line height inside
    /// the editor. Editor-only knob (#0020 pivot) — chat bubbles and chrome
    /// use egui's default line spacing. Renamed from `reading_line_height_mult`.
    #[serde(
        default = "default_editor_line_height_mult",
        alias = "reading_line_height_mult"
    )]
    pub editor_line_height_mult: f32,
    /// Extra letter spacing in pixels for the editor's `TextFormat`. Editor-only
    /// (#0020 pivot). Renamed from `reading_letter_spacing`.
    #[serde(
        default = "default_editor_letter_spacing",
        alias = "reading_letter_spacing"
    )]
    pub editor_letter_spacing: f32,
    /// Maximum prose column width in pixels for the editor (#0020 pivot).
    /// Replaces the hardcoded `MAX_COLUMN_WIDTH` const. The responsive
    /// `MIN_COLUMN_WIDTH` is still applied as a lower bound.
    #[serde(default = "default_editor_column_width")]
    pub editor_column_width: f32,
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
fn default_font_size_normal() -> f32 {
    18.0
}
fn default_font_size_header() -> f32 {
    22.0
}
fn default_font_size_info() -> f32 {
    13.0
}
fn default_editor_line_height_mult() -> f32 {
    1.7
}
fn default_editor_letter_spacing() -> f32 {
    0.4
}
fn default_editor_column_width() -> f32 {
    760.0
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
            font_size_normal: default_font_size_normal(),
            font_size_header: default_font_size_header(),
            font_size_info: default_font_size_info(),
            editor_line_height_mult: default_editor_line_height_mult(),
            editor_letter_spacing: default_editor_letter_spacing(),
            editor_column_width: default_editor_column_width(),
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
