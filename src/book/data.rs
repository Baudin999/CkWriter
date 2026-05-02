//! Writer-facing book data, persisted as `<book-root>/book.json`.
//!
//! This is distinct from `BookConfig` (in `Info/index.json`), which holds
//! technical paths the engine needs to start. `BookData` holds taxonomy the
//! *writer* curates: character categories, relationship kinds, and similar
//! controlled vocabularies the UI offers as dropdowns.
//!
//! Missing file → seeded defaults. The defaults are deliberately
//! genre-flavored (urban fantasy / Erikson-Donaldson) since that is the
//! project's home turf; the writer is expected to edit them per book.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "book.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookData {
    #[serde(default = "default_categories")]
    pub categories: Vec<String>,
    #[serde(default = "default_relation_kinds")]
    pub relation_kinds: Vec<String>,
}

impl Default for BookData {
    fn default() -> Self {
        Self {
            categories: default_categories(),
            relation_kinds: default_relation_kinds(),
        }
    }
}

fn default_categories() -> Vec<String> {
    [
        "Protagonist",
        "Close to Protagonist",
        "Side Character",
        "Antagonist",
        "Antihero",
        "Mentor",
        "Rival",
        "Love Interest",
        "Family",
        "Faction Leader",
        "Soldier",
        "Forsaken",
        "Ascendant",
        "Servant",
        "Walk-on",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn default_relation_kinds() -> Vec<String> {
    [
        "child of",
        "parent of",
        "sibling of",
        "married to",
        "lover of",
        "friend of",
        "ally of",
        "enemy of",
        "rival of",
        "mentor of",
        "student of",
        "works for",
        "leads",
        "loyal to",
        "betrayed by",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

pub fn file_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

impl BookData {
    pub fn load(root: &Path) -> Self {
        let p = file_path(root);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("book.json parse failed ({e}); using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let p = file_path(root);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(&p, s)?;
        Ok(())
    }

    /// Inverse of a relation kind, used when the inspector auto-mirrors a
    /// relation onto the target entity ("child of" ↔ "parent of"). Returns
    /// `None` for kinds whose inverse isn't well-defined or for arbitrary
    /// user-typed kinds — the caller must skip mirroring rather than guess.
    pub fn inverse_relation(&self, kind: &str) -> Option<String> {
        let pair = match kind.trim().to_lowercase().as_str() {
            "child of" => Some("parent of"),
            "parent of" => Some("child of"),
            "sibling of" => Some("sibling of"),
            "married to" => Some("married to"),
            "lover of" => Some("lover of"),
            "friend of" => Some("friend of"),
            "ally of" => Some("ally of"),
            "enemy of" => Some("enemy of"),
            "rival of" => Some("rival of"),
            "mentor of" => Some("student of"),
            "student of" => Some("mentor of"),
            "works for" => Some("leads"),
            "leads" => Some("works for"),
            _ => None,
        };
        pair.map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_seeded_defaults() {
        let dir = tempdir();
        let data = BookData::load(&dir);
        assert!(data.categories.contains(&"Protagonist".to_string()));
        assert!(data.categories.contains(&"Forsaken".to_string()));
        assert!(data.relation_kinds.contains(&"child of".to_string()));
    }

    #[test]
    fn round_trip_preserves_edits() {
        let dir = tempdir();
        let mut data = BookData::default();
        data.categories.push("Bonecaster".into());
        data.relation_kinds.push("haunted by".into());
        data.save(&dir).expect("save");

        let reloaded = BookData::load(&dir);
        assert!(reloaded.categories.contains(&"Bonecaster".to_string()));
        assert!(reloaded.relation_kinds.contains(&"haunted by".to_string()));
    }

    #[test]
    fn inverse_relation_pairs_what_it_should_and_skips_what_it_should() {
        let d = BookData::default();
        assert_eq!(d.inverse_relation("child of").as_deref(), Some("parent of"));
        assert_eq!(d.inverse_relation("parent of").as_deref(), Some("child of"));
        assert_eq!(d.inverse_relation("sibling of").as_deref(), Some("sibling of"));
        assert_eq!(d.inverse_relation("works for").as_deref(), Some("leads"));
        assert_eq!(d.inverse_relation("leads").as_deref(), Some("works for"));
        assert_eq!(d.inverse_relation("Child Of").as_deref(), Some("parent of"));
        // "loyal to" intentionally has no inverse — loyalty isn't mutual.
        assert_eq!(d.inverse_relation("loyal to"), None);
        // Free-text user kind: don't guess.
        assert_eq!(d.inverse_relation("haunted by"), None);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let dir = tempdir();
        std::fs::write(file_path(&dir), "{ this is not json").unwrap();
        let data = BookData::load(&dir);
        assert!(!data.categories.is_empty());
    }

    /// Per-test sandbox under the OS temp dir. Manual rather than the `tempfile`
    /// crate to avoid a new dependency for two tests.
    fn tempdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ckwriter-bookdata-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
