use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    #[serde(default)]
    pub recent_books: Vec<PathBuf>,
    #[serde(default = "default_font_size")]
    pub editor_font_size: f32,
    #[serde(default)]
    pub last_book: Option<PathBuf>,
    #[serde(default)]
    pub last_chapter: Option<PathBuf>,
}

fn default_model() -> String {
    "gemma4:latest".into()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}
fn default_font_size() -> f32 {
    16.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model(),
            ollama_url: default_ollama_url(),
            recent_books: Vec::new(),
            editor_font_size: default_font_size(),
            last_book: None,
            last_chapter: None,
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
