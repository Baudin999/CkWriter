//! Per-chapter coach suggestion lifecycle, persisted to
//! `<book-root>/Info/suggestions/<folder>/<name>.json`.
//!
//! Each chapter's file holds a `BTreeMap<id, SuggestionRecord>` keyed by a
//! deterministic identity hash (`blake3(pipeline + paragraph_id + normalized_quote)`),
//! so re-running a pipeline that produces the same flag against the same
//! paragraph dedupes onto the existing record. Lifecycle states are explicit:
//!
//! - **Proposed** — fresh from the model, hasn't been accepted or dismissed
//! - **Accepted** — writer applied the suggestion; text was rewritten
//! - **Dismissed** — writer rejected the flag (durable intent)
//! - **Stale** — the paragraph the flag anchored to has been rewritten or removed
//!
//! Auto-stale fires on chapter open and after each pipeline ingest; only
//! Proposed records with a known `paragraph_id` are eligible (anchor failures
//! at ingest time live forever — the writer can still accept/dismiss them by
//! hand).

use crate::book::dismissals::normalize as normalize_quote;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Proposed,
    Accepted,
    Dismissed,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionRecord {
    pub id: String,
    pub pipeline: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub paragraph_id: Option<String>,
    pub quote: String,
    pub normalized_quote: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub suggestion: String,
    pub status: Status,
    pub created_at: i64,
    #[serde(default)]
    pub resolved_at: Option<i64>,
    /// Writer-supplied rationale for dismissing this flag (#0027). Threaded
    /// into the AI prompt's "Already reviewed" section so the model sees the
    /// reasoning, not just the quote — lets it generalize across paraphrases
    /// the string-similarity dedup can't catch.
    ///
    /// `None` for non-Dismissed records and for Dismissed records the writer
    /// hasn't annotated yet. Empty `Some("")` collapses to "no note rendered"
    /// the same as `None` at prompt-build time.
    #[serde(default)]
    pub dismissal_note: Option<String>,
}

/// On-disk file for a single chapter's lifecycle records.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChapterSuggestions {
    /// Keyed by `SuggestionRecord::id`. Map (not Vec) so identity dedupe is
    /// O(log n) per ingested flag and the on-disk shape is human-scannable.
    pub records: BTreeMap<String, SuggestionRecord>,
}

pub fn file_path(root: &Path, folder: &str, name: &str) -> PathBuf {
    root.join("Info")
        .join("suggestions")
        .join(folder)
        .join(format!("{name}.json"))
}

impl ChapterSuggestions {
    pub fn load(root: &Path, folder: &str, name: &str) -> Self {
        let p = file_path(root, folder, name);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!(
                    "suggestions {}/{} parse failed ({e}); using empty",
                    folder,
                    name
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, root: &Path, folder: &str, name: &str) -> Result<()> {
        let p = file_path(root, folder, name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(&self)?;
        std::fs::write(&p, s)?;
        Ok(())
    }
}

/// Lazy per-chapter cache backed by `Info/suggestions/<folder>/<name>.json`.
/// Held on `Book` so accept/dismiss/stale paths share state across panel
/// renders without re-reading the file on every keystroke.
#[derive(Debug, Default)]
pub struct SuggestionStore {
    cache: HashMap<(String, String), ChapterSuggestions>,
}

impl SuggestionStore {
    /// Get-or-load the chapter's suggestions, creating an empty one if the
    /// file doesn't exist. Returned reference is mutable so callers can update
    /// records in place; persist with `save_chapter` after.
    pub fn for_chapter_mut(
        &mut self,
        root: &Path,
        folder: &str,
        name: &str,
    ) -> &mut ChapterSuggestions {
        let key = (folder.to_string(), name.to_string());
        self.cache
            .entry(key)
            .or_insert_with(|| ChapterSuggestions::load(root, folder, name))
    }

    pub fn for_chapter(&self, folder: &str, name: &str) -> Option<&ChapterSuggestions> {
        self.cache
            .get(&(folder.to_string(), name.to_string()))
    }

    pub fn save_chapter(&self, root: &Path, folder: &str, name: &str) -> Result<()> {
        if let Some(c) = self.for_chapter(folder, name) {
            c.save(root, folder, name)?;
        }
        Ok(())
    }

    /// Drop the in-memory cache for a chapter. Called when the writer leaves
    /// a chapter so subsequent re-opens load from disk afresh — guards against
    /// a future code path that mutates the file outside the cache (e.g. an
    /// external sync). Fine to call on a chapter that was never loaded.
    #[allow(dead_code)]
    pub fn drop_chapter(&mut self, folder: &str, name: &str) {
        self.cache
            .remove(&(folder.to_string(), name.to_string()));
    }
}

/// Identity hash for a flag. Stable across pipeline re-runs (same paragraph,
/// same normalized quote → same id), so re-ingest dedupes onto the existing
/// record and preserves status history.
pub fn id_hash(pipeline: &str, paragraph_id: Option<&str>, normalized_quote: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"ckwriter-suggestion-id-v1");
    h.update(pipeline.as_bytes());
    h.update(b"|");
    h.update(paragraph_id.unwrap_or("").as_bytes());
    h.update(b"|");
    h.update(normalized_quote.as_bytes());
    h.finalize().to_hex().to_string()
}

/// Threshold for token-set Jaccard similarity (#0025). Hand-tuned against
/// the `Ancient/Wua.json` regression: dismissed and re-flagged quotes that
/// cover the same observation typically score ≥ 0.7 once one isn't a strict
/// substring of the other; genuinely different observations land below.
pub const FUZZY_JACCARD_THRESHOLD: f32 = 0.7;

/// Fuzzy lookup: given an incoming flag, find the existing record id (if any)
/// that covers the same observation. Used by `ingest_response` to avoid
/// piling up parallel Proposed records when the model picks a different
/// quote substring for a flag that's already Dismissed/Accepted/Proposed.
///
/// A record matches when scoped to the same `(pipeline, paragraph_id)` AND
/// either:
///  - one normalized quote is a substring of the other (catches the common
///    "model returned a shorter span" case), OR
///  - the token-set Jaccard score over normalized whitespace-split tokens
///    meets `FUZZY_JACCARD_THRESHOLD` (catches reorderings and minor word
///    edits).
///
/// Stale records are excluded — they're auto-swept tombstones, not deliberate
/// writer decisions, and matching against them would silently swallow new
/// flags on a paragraph the writer rewrote.
///
/// Returns the id of the highest-scoring match, ties broken by Jaccard then
/// by lexicographic id (deterministic).
pub fn fuzzy_match_record_id(
    chapter: &ChapterSuggestions,
    pipeline: &str,
    paragraph_id: Option<&str>,
    normalized_quote: &str,
) -> Option<String> {
    if normalized_quote.is_empty() {
        return None;
    }
    let new_tokens: std::collections::HashSet<&str> =
        normalized_quote.split_whitespace().collect();
    if new_tokens.is_empty() {
        return None;
    }

    let mut best: Option<(f32, &str)> = None;
    for rec in chapter.records.values() {
        if rec.status == Status::Stale {
            continue;
        }
        if rec.pipeline != pipeline {
            continue;
        }
        if rec.paragraph_id.as_deref() != paragraph_id {
            continue;
        }
        let existing = rec.normalized_quote.as_str();
        if existing.is_empty() {
            continue;
        }

        let substring_match = existing.contains(normalized_quote)
            || normalized_quote.contains(existing);
        let existing_tokens: std::collections::HashSet<&str> =
            existing.split_whitespace().collect();
        let jaccard = if existing_tokens.is_empty() {
            0.0
        } else {
            let intersection = new_tokens.intersection(&existing_tokens).count() as f32;
            let union = new_tokens.union(&existing_tokens).count() as f32;
            intersection / union
        };

        if substring_match || jaccard >= FUZZY_JACCARD_THRESHOLD {
            // Substring matches always beat pure-Jaccard ones; among
            // pure-Jaccard ties we keep the highest-scoring then the
            // lexicographically-smallest id so the choice is stable across
            // runs (BTreeMap iteration is sorted, but we still pin it
            // explicitly because two records can have the same Jaccard).
            let score = if substring_match { 1.0 } else { jaccard };
            match best {
                None => best = Some((score, rec.id.as_str())),
                Some((bscore, bid)) => {
                    let better = score > bscore
                        || (score == bscore && rec.id.as_str() < bid);
                    if better {
                        best = Some((score, rec.id.as_str()));
                    }
                }
            }
        }
    }
    best.map(|(_, id)| id.to_string())
}

/// Sweep `Proposed` records: any whose anchored paragraph has been rewritten
/// (or removed) becomes `Stale`. Returns `true` iff any record was changed,
/// so callers can decide whether to persist.
///
/// `paragraphs` and `editor_text` are the just-parsed live state. We use byte
/// ranges from the paragraphs to extract the current paragraph text and check
/// whether `normalized_quote` is still a substring after running it through
/// the same `dismissals::normalize` rules used to build the quote in the first
/// place.
pub fn auto_stale(
    chapter: &mut ChapterSuggestions,
    paragraphs: &[crate::book::paragraphs::Paragraph],
    editor_text: &str,
    now_unix: i64,
) -> bool {
    let by_id: HashMap<&str, &crate::book::paragraphs::Paragraph> =
        paragraphs.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut changed = false;
    for rec in chapter.records.values_mut() {
        if rec.status != Status::Proposed {
            continue;
        }
        let Some(pid) = rec.paragraph_id.as_deref() else {
            continue;
        };
        let stale = match by_id.get(pid) {
            None => true,
            Some(p) => {
                let (s, e) = p.char_range;
                let para_text = editor_text.get(s..e).unwrap_or("");
                let normalized_para = normalize_quote(para_text);
                !normalized_para.contains(&rec.normalized_quote)
            }
        };
        if stale {
            rec.status = Status::Stale;
            rec.resolved_at = Some(now_unix);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::paragraphs::{parse_and_match, Paragraph};

    fn tempdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ckwriter-suggestions-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_record(id: &str, paragraph_id: Option<&str>, quote: &str, status: Status) -> SuggestionRecord {
        SuggestionRecord {
            id: id.to_string(),
            pipeline: "voice".into(),
            kind: String::new(),
            paragraph_id: paragraph_id.map(|s| s.to_string()),
            quote: quote.to_string(),
            normalized_quote: normalize_quote(quote),
            why: String::new(),
            suggestion: String::new(),
            status,
            created_at: 1_700_000_000,
            resolved_at: None,
            dismissal_note: None,
        }
    }

    #[test]
    fn id_hash_is_stable_for_same_inputs() {
        let a = id_hash("voice", Some("p_aaaaaaaa"), "the dog ran");
        let b = id_hash("voice", Some("p_aaaaaaaa"), "the dog ran");
        assert_eq!(a, b);
    }

    #[test]
    fn id_hash_changes_with_pipeline() {
        let a = id_hash("voice", Some("p_aaaaaaaa"), "the dog ran");
        let b = id_hash("prose", Some("p_aaaaaaaa"), "the dog ran");
        assert_ne!(a, b);
    }

    #[test]
    fn id_hash_changes_with_paragraph_id() {
        let a = id_hash("voice", Some("p_aaaaaaaa"), "the dog ran");
        let b = id_hash("voice", Some("p_bbbbbbbb"), "the dog ran");
        assert_ne!(a, b);
    }

    #[test]
    fn id_hash_changes_with_quote() {
        let a = id_hash("voice", Some("p_aaaaaaaa"), "the dog ran");
        let b = id_hash("voice", Some("p_aaaaaaaa"), "the dog jumped");
        assert_ne!(a, b);
    }

    #[test]
    fn id_hash_distinguishes_some_vs_none_paragraph() {
        let a = id_hash("voice", None, "the dog ran");
        let b = id_hash("voice", Some(""), "the dog ran");
        // Both feed an empty string into the hash — they collide. Acceptable
        // because paragraph_id is never the empty string in practice (they
        // start with `p_`); Some("") would only happen via corruption.
        assert_eq!(a, b);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempdir();
        let mut c = ChapterSuggestions::default();
        let r = make_record("hash1", Some("p_12345678"), "hello world", Status::Proposed);
        c.records.insert(r.id.clone(), r);
        c.save(&dir, "Modern", "Awakening").unwrap();
        let loaded = ChapterSuggestions::load(&dir, "Modern", "Awakening");
        assert_eq!(loaded.records.len(), 1);
        let rec = loaded.records.values().next().unwrap();
        assert_eq!(rec.quote, "hello world");
        assert_eq!(rec.status, Status::Proposed);
    }

    #[test]
    fn missing_file_yields_empty_chapter() {
        let dir = tempdir();
        let c = ChapterSuggestions::load(&dir, "Modern", "Nope");
        assert!(c.records.is_empty());
    }

    #[test]
    fn dismissal_note_round_trips_through_chapter_store() {
        // #0027: writer-supplied rationale for a dismissed flag must survive
        // save → load. Records without a note (legacy data + freshly proposed
        // flags) must deserialize as `None`, not error.
        let dir = tempdir();
        let mut c = ChapterSuggestions::default();
        let mut annotated = make_record("h1", Some("p_aaaaaaaa"), "really tired", Status::Dismissed);
        annotated.dismissal_note =
            Some("colloquial register is intentional for this character".to_string());
        c.records.insert(annotated.id.clone(), annotated);
        let plain = make_record("h2", Some("p_aaaaaaaa"), "the cat sat", Status::Proposed);
        c.records.insert(plain.id.clone(), plain);
        c.save(&dir, "Modern", "WithNotes").unwrap();

        let loaded = ChapterSuggestions::load(&dir, "Modern", "WithNotes");
        let by_id: BTreeMap<String, SuggestionRecord> = loaded
            .records
            .values()
            .map(|r| (r.id.clone(), r.clone()))
            .collect();
        assert_eq!(
            by_id.get("h1").and_then(|r| r.dismissal_note.clone()),
            Some("colloquial register is intentional for this character".to_string()),
        );
        assert_eq!(by_id.get("h2").and_then(|r| r.dismissal_note.clone()), None);
    }

    #[test]
    fn legacy_record_without_dismissal_note_field_loads_none() {
        // Sidecars written before #0027 have no `dismissal_note` field on
        // their records. serde(default) must turn that into `None`, not an
        // unknown-field error or a parse failure.
        let dir = tempdir();
        let p = file_path(&dir, "Modern", "Legacy");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            r#"{"h1":{"id":"h1","pipeline":"prose","quote":"q","normalized_quote":"q","status":"dismissed","created_at":1}}"#,
        )
        .unwrap();
        let loaded = ChapterSuggestions::load(&dir, "Modern", "Legacy");
        assert_eq!(loaded.records.len(), 1);
        let rec = loaded.records.values().next().unwrap();
        assert_eq!(rec.dismissal_note, None);
    }

    #[test]
    fn auto_stale_marks_when_paragraph_removed() {
        let src = "the dog ran fast across the open field hunting rabbits in the rain.\n";
        let parsed = parse_and_match(src, &[]);
        let pid = parsed[0].id.clone();

        let mut c = ChapterSuggestions::default();
        let r = make_record("h1", Some(&pid), "across the open field", Status::Proposed);
        c.records.insert(r.id.clone(), r);

        // Now the chapter has DIFFERENT paragraphs — the prior id is gone.
        let new_src = "completely different prose appears here in the chapter today.\n";
        let new_parsed = parse_and_match(new_src, &[]);
        assert_ne!(new_parsed[0].id, pid);

        let changed = auto_stale(&mut c, &new_parsed, new_src, 42);
        assert!(changed);
        let rec = c.records.values().next().unwrap();
        assert_eq!(rec.status, Status::Stale);
        assert_eq!(rec.resolved_at, Some(42));
    }

    #[test]
    fn auto_stale_marks_when_quote_disappeared_from_same_paragraph() {
        // Build a long enough paragraph that an edit doesn't change its id
        // (so the paragraph_id still points at the same paragraph in the new
        // text). Its normalized text changes, so the quote disappears.
        let p1 = "the dog ran across the open field with surprising speed considering the heavy rain";
        let parsed = parse_and_match(p1, &[]);
        let pid = parsed[0].id.clone();
        let prior_paragraphs: Vec<Paragraph> = parsed.clone();

        let mut c = ChapterSuggestions::default();
        let r = make_record("h1", Some(&pid), "with surprising speed", Status::Proposed);
        c.records.insert(r.id.clone(), r);

        // Edit the paragraph: drop the quote substring entirely. Long enough
        // that Jaccard still matches and the paragraph id is preserved.
        let p2 = "the dog ran across the open field steadily considering the heavy rain falling on him today";
        let new_parsed = parse_and_match(p2, &prior_paragraphs);
        assert_eq!(new_parsed[0].id, pid, "paragraph id should be preserved");

        let changed = auto_stale(&mut c, &new_parsed, p2, 99);
        assert!(changed);
        let rec = c.records.values().next().unwrap();
        assert_eq!(rec.status, Status::Stale);
    }

    #[test]
    fn auto_stale_skips_records_without_paragraph_id() {
        let src = "anything\n";
        let parsed = parse_and_match(src, &[]);
        let mut c = ChapterSuggestions::default();
        let r = make_record("h1", None, "anything", Status::Proposed);
        c.records.insert(r.id.clone(), r);
        let changed = auto_stale(&mut c, &parsed, src, 1);
        assert!(!changed);
        let rec = c.records.values().next().unwrap();
        assert_eq!(rec.status, Status::Proposed);
    }

    #[test]
    fn auto_stale_does_not_touch_non_proposed_records() {
        let src = "fresh\n";
        let parsed = parse_and_match(src, &[]);
        let mut c = ChapterSuggestions::default();
        let mut accepted = make_record("h1", Some("p_dead0000"), "vanished", Status::Accepted);
        accepted.resolved_at = Some(1_700_000_001);
        c.records.insert(accepted.id.clone(), accepted);
        let dismissed = make_record("h2", Some("p_dead1111"), "gone", Status::Dismissed);
        c.records.insert(dismissed.id.clone(), dismissed);
        let changed = auto_stale(&mut c, &parsed, src, 99);
        assert!(!changed);
        for rec in c.records.values() {
            assert_ne!(rec.status, Status::Stale);
        }
    }

    // --- fuzzy_match_record_id (#0025) -------------------------------------

    fn rec_with(
        id: &str,
        pipeline: &str,
        paragraph_id: Option<&str>,
        quote: &str,
        status: Status,
    ) -> SuggestionRecord {
        SuggestionRecord {
            id: id.to_string(),
            pipeline: pipeline.to_string(),
            kind: String::new(),
            paragraph_id: paragraph_id.map(|s| s.to_string()),
            quote: quote.to_string(),
            normalized_quote: normalize_quote(quote),
            why: String::new(),
            suggestion: String::new(),
            status,
            created_at: 1,
            resolved_at: None,
            dismissal_note: None,
        }
    }

    fn store_with(records: Vec<SuggestionRecord>) -> ChapterSuggestions {
        let mut c = ChapterSuggestions::default();
        for r in records {
            c.records.insert(r.id.clone(), r);
        }
        c
    }

    #[test]
    fn fuzzy_match_returns_none_on_empty_store() {
        let c = ChapterSuggestions::default();
        let hit = fuzzy_match_record_id(&c, "prose", Some("p_x"), "anything");
        assert!(hit.is_none());
    }

    #[test]
    fn fuzzy_match_returns_none_on_empty_quote() {
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            "the dog ran",
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(&c, "prose", Some("p_x"), "");
        assert!(hit.is_none());
    }

    #[test]
    fn fuzzy_match_catches_strict_substring_of_existing() {
        // Mirrors the real Wua.json regression: dismissed quote is a long
        // sentence; the model re-ran and returned a strict substring of it.
        let dismissed_quote =
            "yet, others manage to escape it all, swirling through life while evading monotony and servitude.";
        let new_quote = "swirling through life while evading monotony and servitude";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_2bd65496"),
            dismissed_quote,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_2bd65496"),
            &normalize_quote(new_quote),
        );
        assert_eq!(hit.as_deref(), Some("h1"));
    }

    #[test]
    fn fuzzy_match_catches_existing_substring_of_new() {
        // Reverse: the model returned a longer span that contains the
        // dismissed quote.
        let dismissed = "swirling through life";
        let new = "swirling through life while evading monotony";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            dismissed,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_x"),
            &normalize_quote(new),
        );
        assert_eq!(hit.as_deref(), Some("h1"));
    }

    #[test]
    fn fuzzy_match_catches_token_jaccard_above_threshold() {
        // Same observation, slightly different word order / punctuation —
        // neither is a substring of the other, but token-set Jaccard is high.
        let dismissed = "choking on their dust gasping for breath";
        let new = "gasping for breath, choking on their dust";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            dismissed,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_x"),
            &normalize_quote(new),
        );
        assert_eq!(hit.as_deref(), Some("h1"));
    }

    #[test]
    fn fuzzy_match_misses_below_jaccard_threshold() {
        // Genuinely different observations — share a few common words but
        // not enough to push Jaccard above 0.7 and no substring overlap.
        let dismissed = "the moon hung low over the silent harbour";
        let new = "the captain shouted orders from the bridge";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            dismissed,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_x"),
            &normalize_quote(new),
        );
        assert!(hit.is_none(), "low-overlap quotes must not match: got {hit:?}");
    }

    #[test]
    fn fuzzy_match_scopes_to_pipeline() {
        // Same paragraph, same quote, but a different pipeline's record —
        // must not match (a prose flag and a spelling flag on the same words
        // are different observations).
        let q = "the dog ran across the field";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            q,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "spelling",
            Some("p_x"),
            &normalize_quote(q),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn fuzzy_match_scopes_to_paragraph() {
        // Same pipeline, same quote, different paragraph — must not match.
        let q = "the dog ran across the field";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_a"),
            q,
            Status::Dismissed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_b"),
            &normalize_quote(q),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn fuzzy_match_skips_stale_records() {
        // Stale records are auto-swept tombstones — matching against them
        // would silently swallow new flags on a paragraph the writer
        // rewrote.
        let q = "the dog ran across the field";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            q,
            Status::Stale,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_x"),
            &normalize_quote(q),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn fuzzy_match_matches_against_proposed_too() {
        // Not just dismissed: an existing Proposed record from a prior
        // partial run should also dedupe so we don't double-report.
        let q1 = "the dog ran";
        let q2 = "the dog ran fast across the field";
        let c = store_with(vec![rec_with(
            "h1",
            "prose",
            Some("p_x"),
            q1,
            Status::Proposed,
        )]);
        let hit = fuzzy_match_record_id(
            &c,
            "prose",
            Some("p_x"),
            &normalize_quote(q2),
        );
        assert_eq!(hit.as_deref(), Some("h1"));
    }

    #[test]
    fn store_lazy_loads_and_caches() {
        let dir = tempdir();
        let mut store = SuggestionStore::default();
        // First access creates an empty in-memory chapter.
        {
            let c = store.for_chapter_mut(&dir, "Modern", "Fresh");
            assert!(c.records.is_empty());
            let r = make_record("h1", None, "x", Status::Proposed);
            c.records.insert(r.id.clone(), r);
        }
        // Re-access returns the same in-memory state without going to disk.
        let c2 = store.for_chapter_mut(&dir, "Modern", "Fresh");
        assert_eq!(c2.records.len(), 1);
    }
}
