//! Per-chapter metadata, persisted to
//! `<book-root>/Info/chapters/<folder>/<name>.json`.
//!
//! Keyed by the stable CamelCase `name` (no number prefix), so renumbering or
//! DnD reorders preserve metadata. A chapter file `Modern/010_Awakening.tex`
//! has its sidecar at `Info/chapters/Modern/Awakening.json`.
//!
//! This is the schema container that #0002–#0006 hang off (paragraph index,
//! per-paragraph caching, locks, embeddings). Fields the writer doesn't edit
//! today (`pov`, `tags`) are storage-only in v1 — UI lands when those tickets
//! land.

use crate::book::paragraphs::ParagraphMeta;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterMeta {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub goals: String,
    #[serde(default)]
    pub plot_notes: String,
    /// Entity id of the POV character. Storage only in v1.
    #[serde(default)]
    pub pov: Option<String>,
    /// Free-form tags. Storage only in v1.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Word count of the prose-stripped chapter body, recomputed on save.
    #[serde(default)]
    pub word_count: usize,
    /// Most recent score from the voice pipeline. `i32` to match
    /// `llm::revision::RawVoice.score`; the model never returns negatives in
    /// practice but the type already exists.
    #[serde(default)]
    pub voice_score: Option<i32>,
    /// Unix seconds when the voice pipeline last ran successfully against
    /// this chapter.
    #[serde(default)]
    pub last_coached_at: Option<i64>,
    /// Stable per-paragraph index — id + content hash, in source order. The
    /// substrate that #0003–#0005 hang off; recomputed on chapter open and on
    /// save. See `book::paragraphs` for the splitter and matcher.
    #[serde(default)]
    pub paragraphs: Vec<ParagraphMeta>,
}

pub fn file_path(root: &Path, folder: &str, name: &str) -> PathBuf {
    root.join("Info")
        .join("chapters")
        .join(folder)
        .join(format!("{name}.json"))
}

/// Load metadata for the given chapter. Missing file → defaults. Malformed
/// JSON → defaults with a warning, mirroring `manuscript.json` behaviour.
pub fn load(root: &Path, folder: &str, name: &str) -> ChapterMeta {
    let p = file_path(root, folder, name);
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!(
                "chapter meta {}/{} parse failed ({e}); using defaults",
                folder,
                name
            );
            ChapterMeta::default()
        }),
        Err(_) => ChapterMeta::default(),
    }
}

pub fn save(root: &Path, folder: &str, name: &str, meta: &ChapterMeta) -> Result<()> {
    let p = file_path(root, folder, name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(meta)?;
    std::fs::write(&p, s)?;
    Ok(())
}

/// Word count of the prose-stripped chapter body. Whitespace-separated
/// runs of non-whitespace count as one word.
pub fn word_count_from_prose(prose: &str) -> usize {
    prose.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ckwriter-chapter-meta-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir();
        let meta = ChapterMeta {
            summary: "Hero arrives in town.".into(),
            goals: "Establish stakes.".into(),
            plot_notes: "Maybe rain?".into(),
            pov: Some("char_arn".into()),
            tags: vec!["opening".into(), "modern".into()],
            word_count: 1234,
            voice_score: Some(72),
            last_coached_at: Some(1_700_000_000),
            paragraphs: vec![
                ParagraphMeta {
                    id: "p_12345678".into(),
                    hash: "0123456789abcdef".into(),
                },
                ParagraphMeta {
                    id: "p_abcdef01".into(),
                    hash: "fedcba9876543210".into(),
                },
            ],
        };
        save(&dir, "Modern", "Awakening", &meta).expect("save");
        let loaded = load(&dir, "Modern", "Awakening");
        assert_eq!(loaded, meta);
    }

    #[test]
    fn legacy_meta_without_paragraphs_loads_empty_index() {
        // A sidecar written before #0002 has no `paragraphs` field at all.
        // serde(default) must turn that into an empty Vec, not a parse error.
        let dir = tempdir();
        let p = file_path(&dir, "Modern", "Legacy");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            r#"{"summary": "carryover", "word_count": 12}"#,
        )
        .unwrap();
        let meta = load(&dir, "Modern", "Legacy");
        assert_eq!(meta.summary, "carryover");
        assert_eq!(meta.word_count, 12);
        assert!(meta.paragraphs.is_empty());
    }

    #[test]
    fn missing_file_yields_default() {
        let dir = tempdir();
        let meta = load(&dir, "Modern", "Nonexistent");
        assert_eq!(meta, ChapterMeta::default());
    }

    #[test]
    fn malformed_json_falls_back_to_default() {
        let dir = tempdir();
        let p = file_path(&dir, "Modern", "Broken");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ this is not valid json").unwrap();
        let meta = load(&dir, "Modern", "Broken");
        assert_eq!(meta, ChapterMeta::default());
    }

    #[test]
    fn save_creates_nested_folder() {
        let dir = tempdir();
        // Confirm we don't require the Info/chapters/Modern path to pre-exist.
        let meta = ChapterMeta {
            word_count: 7,
            ..Default::default()
        };
        save(&dir, "Modern", "FreshChapter", &meta).expect("save");
        let p = file_path(&dir, "Modern", "FreshChapter");
        assert!(p.exists());
    }

    #[test]
    fn word_count_handles_whitespace_runs() {
        assert_eq!(word_count_from_prose(""), 0);
        assert_eq!(word_count_from_prose("   "), 0);
        assert_eq!(word_count_from_prose("one"), 1);
        assert_eq!(word_count_from_prose("one  two\tthree\nfour"), 4);
    }

    #[test]
    fn unknown_fields_do_not_break_load() {
        let dir = tempdir();
        let p = file_path(&dir, "Modern", "Future");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // Simulates a forward-compatible field a later ticket might add.
        std::fs::write(
            &p,
            r#"{"summary": "ok", "paragraphs": [{"id": "p1"}], "word_count": 3}"#,
        )
        .unwrap();
        let meta = load(&dir, "Modern", "Future");
        assert_eq!(meta.summary, "ok");
        assert_eq!(meta.word_count, 3);
    }
}
