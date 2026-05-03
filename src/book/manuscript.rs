//! Source of truth for the book's reading order, persisted to
//! `<book-root>/Info/manuscript.json`.
//!
//! The manuscript is an ordered list of `(folder, name)` pairs. The numeric
//! prefix of each chapter file (`010_`, `020_`, …) and its position in the
//! TeX include block are *derived* from this list — never the other way
//! around. That keeps a DnD reorder atomic: rewrite the JSON, recompute
//! filenames, rewrite `main.tex`. The JSON is the only thing humans need to
//! reason about.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "Info/manuscript.json";

/// Number step used when assigning filename prefixes from manuscript position.
/// Chosen so that an Ancient slice of (Arrival, Wua, FirstEncounter) produces
/// `010_Arrival`, `020_Wua`, `030_FirstEncounter` — leaving room between
/// numbers in case the writer ever ls's the folder and wants to slot something
/// in by hand.
pub const NUMBER_STRIDE: u32 = 10;

/// Folders under the book root whose `.tex` files CkWriter manages. A file
/// in any other directory is invisible to add/delete/reorder; a file inside
/// these folders is either a manuscript chapter (numbered) or an orphan
/// (un-numbered, parked). Hardcoded for v1; will move to BookConfig if a
/// second project ever needs different folders.
pub const MANAGED_FOLDERS: &[&str] = &["Ancient", "Modern"];

/// One slot in the manuscript. The folder is the directory under the book
/// root (e.g. `Ancient`, `Modern`); `name` is the CamelCase identifier that
/// becomes the filename suffix (e.g. `Arrival` → `010_Arrival.tex`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterRef {
    pub folder: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manuscript {
    #[serde(default)]
    pub chapters: Vec<ChapterRef>,
}

pub fn file_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

impl Manuscript {
    pub fn load(root: &Path) -> Self {
        let p = file_path(root);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("manuscript.json parse failed ({e}); using empty");
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

    pub fn position(&self, folder: &str, name: &str) -> Option<usize> {
        self.chapters
            .iter()
            .position(|c| c.folder == folder && c.name == name)
    }

    pub fn contains(&self, folder: &str, name: &str) -> bool {
        self.position(folder, name).is_some()
    }
}

/// Filename for a chapter at `folder_pos` inside its folder, e.g.
/// `(0, "Arrival")` → `010_Arrival.tex`.
pub fn derive_filename(folder_pos: usize, name: &str) -> String {
    let n = (folder_pos as u32 + 1) * NUMBER_STRIDE;
    format!("{n:03}_{name}.tex")
}

/// Include path used in `main.tex`, e.g. `(0, "Ancient", "Arrival")` →
/// `Ancient/010_Arrival`.
pub fn derive_include_path(folder_pos: usize, folder: &str, name: &str) -> String {
    let n = (folder_pos as u32 + 1) * NUMBER_STRIDE;
    format!("{folder}/{n:03}_{name}")
}

/// Convert a free-text title into a CamelCase identifier suitable for a
/// filename. Non-alphanumeric runs become word boundaries; the first
/// alphanumeric of each word is forced to uppercase, but interior letters
/// are preserved as-typed so the function is idempotent on input that's
/// already CamelCase. `"first encounter"` → `"FirstEncounter"`,
/// `"FirstEncounter"` → `"FirstEncounter"`, `"a city in need!"` →
/// `"ACityInNeed"`.
pub fn camel_case(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut at_boundary = true;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            if at_boundary {
                out.extend(ch.to_uppercase());
                at_boundary = false;
            } else {
                out.push(ch);
            }
        } else {
            at_boundary = true;
        }
    }
    out
}

/// Pretty version of a CamelCase identifier for UI fallback when a chapter
/// file has no `\chapter{...}` line yet. Inserts a space before every
/// uppercase letter that follows a lowercase letter, so `"FirstEncounter"`
/// renders as `"First Encounter"`. Acronyms stay glued (`"NYCStreets"` →
/// `"NYCStreets"`); the writer can override with `\chapter{...}`.
pub fn humanize(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch.is_uppercase() && prev_lower {
            out.push(' ');
        }
        out.push(ch);
        prev_lower = ch.is_lowercase();
    }
    out
}

/// Strip a `NNN_` prefix off a filename stem. `"010_FirstEncounter"` →
/// `"FirstEncounter"`. Returns the stem unchanged when no numeric prefix
/// is present.
pub fn strip_number_prefix(stem: &str) -> &str {
    let mut digits = 0usize;
    for (_, c) in stem.char_indices() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if digits > 0 && c == '_' {
            return &stem[digits + 1..];
        } else {
            return stem;
        }
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_filenames_with_stride_10() {
        assert_eq!(derive_filename(0, "Arrival"), "010_Arrival.tex");
        assert_eq!(derive_filename(1, "Wua"), "020_Wua.tex");
        assert_eq!(derive_filename(9, "TenthOne"), "100_TenthOne.tex");
    }

    #[test]
    fn derives_include_paths_with_stride_10() {
        assert_eq!(
            derive_include_path(0, "Ancient", "Arrival"),
            "Ancient/010_Arrival"
        );
        assert_eq!(
            derive_include_path(2, "Modern", "LionsDen"),
            "Modern/030_LionsDen"
        );
    }

    #[test]
    fn camel_cases_titles() {
        assert_eq!(camel_case("first encounter"), "FirstEncounter");
        assert_eq!(camel_case("A City in Need!"), "ACityInNeed");
        assert_eq!(camel_case("  many   spaces  "), "ManySpaces");
        assert_eq!(camel_case("hello-world"), "HelloWorld");
        assert_eq!(camel_case(""), "");
        // Idempotent on already-CamelCase input.
        assert_eq!(camel_case("FirstEncounter"), "FirstEncounter");
        assert_eq!(camel_case("NewYorkCity"), "NewYorkCity");
    }

    #[test]
    fn humanizes_camel_case() {
        assert_eq!(humanize("FirstEncounter"), "First Encounter");
        assert_eq!(humanize("ACityInNeed"), "ACity In Need");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn strips_number_prefix() {
        assert_eq!(strip_number_prefix("010_FirstEncounter"), "FirstEncounter");
        assert_eq!(strip_number_prefix("000_Arrival"), "Arrival");
        assert_eq!(strip_number_prefix("NoNumber"), "NoNumber");
        assert_eq!(strip_number_prefix("01NoUnderscore"), "01NoUnderscore");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir();
        let m = Manuscript {
            chapters: vec![
                ChapterRef {
                    folder: "Ancient".into(),
                    name: "Arrival".into(),
                },
                ChapterRef {
                    folder: "Modern".into(),
                    name: "NewYorkCity".into(),
                },
            ],
        };
        m.save(&dir).expect("save");
        let loaded = Manuscript::load(&dir);
        assert_eq!(loaded.chapters, m.chapters);
    }

    #[test]
    fn missing_file_yields_empty() {
        let dir = tempdir();
        let m = Manuscript::load(&dir);
        assert!(m.chapters.is_empty());
    }

    fn tempdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ckwriter-manuscript-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
