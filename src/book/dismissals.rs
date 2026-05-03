//! Coach dismissals — quotes the writer rejected so we can post-filter them
//! out of future pipeline responses, persisted to
//! `<book-root>/Info/coach-dismissals.json`.
//!
//! Storage shape: `chapter_name -> pipeline_label -> set<normalized_quote>`.
//! Chapter names are the stable CamelCase identifiers from `manuscript.json`,
//! not file paths or display titles, so a renumbering or rename of the on-disk
//! file does not orphan a dismissal list.
//!
//! Quotes are normalized (lowercase, whitespace collapsed, trimmed) before
//! storage and lookup so that minor model variation in quote selection — an
//! extra space, a different capitalization, a curly vs straight apostrophe —
//! still matches a previously dismissed flag.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "Info/coach-dismissals.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dismissals {
    /// chapter name -> pipeline label -> set of normalized quotes
    #[serde(default)]
    pub by_chapter: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

pub fn file_path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

impl Dismissals {
    pub fn load(root: &Path) -> Self {
        let p = file_path(root);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("coach-dismissals.json parse failed ({e}); using empty");
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

    pub fn record(&mut self, chapter_name: &str, pipeline_label: &str, quote: &str) {
        let n = normalize(quote);
        if n.is_empty() {
            return;
        }
        self.by_chapter
            .entry(chapter_name.to_string())
            .or_default()
            .entry(pipeline_label.to_string())
            .or_default()
            .insert(n);
    }

    pub fn is_dismissed(&self, chapter_name: &str, pipeline_label: &str, quote: &str) -> bool {
        let n = normalize(quote);
        if n.is_empty() {
            return false;
        }
        self.by_chapter
            .get(chapter_name)
            .and_then(|by_pipe| by_pipe.get(pipeline_label))
            .is_some_and(|set| set.contains(&n))
    }
}

/// Lowercase, collapse runs of whitespace to a single space, trim. Keeps
/// punctuation as-is — punctuation differences are usually meaningful to the
/// flag (e.g. a missing comma is the whole point of a spelling flag).
pub fn normalize(quote: &str) -> String {
    let mut out = String::with_capacity(quote.len());
    let mut last_was_space = false;
    for ch in quote.chars() {
        if ch.is_whitespace() {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            last_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        assert_eq!(normalize("  Hello   World  "), "hello world");
        assert_eq!(normalize("Tab\there"), "tab here");
        assert_eq!(normalize("LINE\nBREAK"), "line break");
    }

    #[test]
    fn normalize_preserves_punctuation() {
        assert_eq!(normalize("It's, you know."), "it's, you know.");
    }

    #[test]
    fn record_and_lookup_roundtrip() {
        let mut d = Dismissals::default();
        d.record("Awakening", "prose", "  The dog ran  fast  ");
        assert!(d.is_dismissed("Awakening", "prose", "the dog ran fast"));
        assert!(d.is_dismissed("Awakening", "prose", "THE DOG RAN  FAST"));
        assert!(!d.is_dismissed("Awakening", "prose", "the cat ran fast"));
        assert!(!d.is_dismissed("Awakening", "voice", "the dog ran fast"));
        assert!(!d.is_dismissed("OtherChapter", "prose", "the dog ran fast"));
    }

    #[test]
    fn empty_quote_is_noop() {
        let mut d = Dismissals::default();
        d.record("X", "prose", "   ");
        assert!(d.by_chapter.is_empty());
        assert!(!d.is_dismissed("X", "prose", ""));
    }
}
