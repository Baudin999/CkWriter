//! Agent Layer storage primitives. See ticket #0008 §"Agent Layer storage".
//!
//! Lives at `Info/agent/` under the book root. Holds artifacts the model has
//! *figured out* (scene transcodings, chapter rollups, observations, retrieval
//! traces, summaries) without polluting either the manuscript or the canon
//! entity files in `Info/Characters` etc.
//!
//! v1 ships the storage layer only — the skill framework that produces
//! artifacts is #0009, the first concrete skill is #0010, and `mentions.json`
//! is populated by an entity-extraction skill in a later ticket. This module
//! provides empty-default round-trips so future tickets can light up without
//! schema migrations.

// Every public item here is the scaffolding the orchestrator (#0009) and the
// transcoder/rollup skills (#0010, #0011) plug into. Unit tests exercise the
// full surface; the binary itself does not yet, so cargo flags everything as
// dead. Allow at module scope with this single justification rather than
// peppering each item — this entire file ships ahead of its first non-test
// caller by design.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::book::paragraphs::ParagraphMeta;

/// Schema version for every Agent Layer artifact. Bump when an existing field
/// changes meaning or shape. Additive fields use `#[serde(default)]` and do
/// *not* require a bump. See ticket #0008 §"Schema versioning".
pub const CURRENT_SCHEMA: u32 = 1;

/// Cap for `retrieval_traces.json` before FIFO eviction kicks in. See
/// ticket #0008 §"Agent Layer storage".
pub const RETRIEVAL_TRACE_CAP: usize = 1000;

/// Coarse model self-rating. Three buckets, not a float — see ticket #0008
/// design notes ("Why `confidence` as a 3-value enum, not a float?").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    #[default]
    Low,
    Medium,
    High,
}

/// Common provenance metadata embedded in every persisted Agent Layer
/// artifact. Flat composition — every artifact struct has a `meta:
/// AgentArtifactMeta` field. Loaders use `#[serde(flatten)]` if needed for
/// forward-compat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentArtifactMeta {
    pub schema: u32,
    /// blake3-hex-16 over the concatenated source paragraphs. Used by
    /// `is_stale` to detect drift; see `compute_source_hash`.
    pub source_hash: String,
    /// Paragraph IDs (e.g. `p_1a2b3c4d`) the artifact was derived from.
    pub source_refs: Vec<String>,
    pub created_at: i64,
    pub created_by_skill: String,
    pub model_id: String,
    #[serde(default)]
    pub confidence: Confidence,
    /// When `true`, `is_stale` always returns `false`. Authors pin
    /// artifacts they've reviewed and want to keep regardless of source
    /// edits. The pin UI lands in a later ticket; storage support is here.
    #[serde(default)]
    pub pinned: bool,
}

impl AgentArtifactMeta {
    pub fn new(skill: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            source_hash: String::new(),
            source_refs: Vec::new(),
            created_at: now_unix(),
            created_by_skill: skill.into(),
            model_id: model.into(),
            confidence: Confidence::Low,
            pinned: false,
        }
    }
}

/// Compute the composite source hash for an artifact's paragraph references.
///
/// **Domain note (decided in ticket #0008 status notes):** blake3 over
/// `id:hash\n` per ref, truncated to 16 hex (consistent with
/// `paragraphs.rs::HASH_HEX_LEN`). Including the id prevents collisions where
/// two paragraphs could swap IDs but keep the same multiset of hashes.
///
/// Closure-based lookup (`hash_lookup: |id| Option<String>`) lets callers
/// union paragraphs across multiple chapters — the orchestrator (#0009)
/// resolves IDs by walking every loaded chapter's paragraph index. Returns
/// `None` if any ref is unknown to the lookup, which `is_stale` short-circuits
/// to "stale" (a deleted source paragraph is the canonical stale case).
pub fn compute_source_hash<F>(refs: &[String], mut hash_lookup: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut hasher = blake3::Hasher::new();
    for id in refs {
        let h = hash_lookup(id)?;
        hasher.update(id.as_bytes());
        hasher.update(b":");
        hasher.update(h.as_bytes());
        hasher.update(b"\n");
    }
    let hex = hasher.finalize().to_hex();
    Some(hex.as_str()[..16].to_string())
}

/// Returns `true` when any source paragraph has been deleted or its hash has
/// changed since the artifact was written. Pinned artifacts always return
/// `false`.
///
/// **Deviation from the ticket's scope spec (`paragraphs: &[Paragraph]`):** we
/// take `&[ParagraphMeta]` because callers persist the index as
/// `ChapterMeta::paragraphs: Vec<ParagraphMeta>` (see #0002), and the runtime
/// `Paragraph` is rebuilt from text on every parse. Slice-of-meta is what's
/// actually available at the call sites this is shipped for. Recorded in
/// ticket #0008 status notes.
pub fn is_stale(meta: &AgentArtifactMeta, paragraphs: &[ParagraphMeta]) -> bool {
    if meta.pinned {
        return false;
    }
    let lookup = |id: &str| -> Option<String> {
        paragraphs
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.hash.clone())
    };
    match compute_source_hash(&meta.source_refs, lookup) {
        Some(current) => current != meta.source_hash,
        None => true,
    }
}

// ============================================================================
// Mentions index — Info/agent/mentions.json
// ============================================================================

/// Reverse map from `EntityId → Vec<ParagraphId>`. Built by an entity-
/// extraction skill in a later ticket; v1 ships storage and an empty default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mentions {
    pub schema: u32,
    #[serde(default)]
    pub by_entity: BTreeMap<String, Vec<String>>,
}

impl Default for Mentions {
    fn default() -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            by_entity: BTreeMap::new(),
        }
    }
}

// ============================================================================
// Per-artifact-kind structs (skeletons — fields populated by later tickets)
// ============================================================================

/// One scene's worth of structured beats — populated by #0010.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneRecord {
    pub meta: AgentArtifactMeta,
    pub scene_id: String,
    pub chapter: String,
    #[serde(default)]
    pub beats: Vec<String>,
}

/// One chapter's rolled-up summary, voice notes, and continuity highlights —
/// populated by #0011.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterRollup {
    pub meta: AgentArtifactMeta,
    pub chapter: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub voice_notes: String,
    #[serde(default)]
    pub continuity_flags: Vec<String>,
}

/// A single observation the model wants to remember — appended to
/// `observations.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub meta: AgentArtifactMeta,
    pub topic: String,
    pub body: String,
}

/// One round-trip of vector retrieval — appended to `retrieval_traces.json`
/// with FIFO eviction at `RETRIEVAL_TRACE_CAP`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalTrace {
    pub meta: AgentArtifactMeta,
    pub query: String,
    #[serde(default)]
    pub returned_paragraph_ids: Vec<String>,
}

/// Content-addressable summary, keyed by `topic_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub meta: AgentArtifactMeta,
    pub topic_hash: String,
    pub topic: String,
    pub body: String,
}

// ============================================================================
// AgentLayer — the runtime handle wired into Book
// ============================================================================

/// On-disk layout owner. Holds the agent root, a cached `Mentions` index, and
/// helpers for reading/writing artifacts. Created by `Book::open`.
pub struct AgentLayer {
    /// Absolute path to the book's `Info/agent/` directory.
    pub root: PathBuf,
    /// Cached `Mentions` index. Loaded on `open`; written through by
    /// `set_mentions`.
    pub mentions: Mentions,
}

impl AgentLayer {
    /// Open the agent layer for a book root, creating the directory tree if
    /// missing. Idempotent.
    pub fn open(book_root: &Path) -> Result<Self> {
        let root = book_root.join("Info").join("agent");
        std::fs::create_dir_all(&root)?;
        // Sub-dirs the artifact writers will need; cheap to create up front
        // so callers don't have to remember. mkdir -p is idempotent.
        for sub in ["scene_records", "chapter_rollups", "summaries"] {
            std::fs::create_dir_all(root.join(sub))?;
        }
        let mentions = load_mentions(&root);
        Ok(Self { root, mentions })
    }

    pub fn mentions_path(&self) -> PathBuf {
        self.root.join("mentions.json")
    }

    /// Look up paragraph references for an entity. Empty when the entity has
    /// no recorded mentions or the index hasn't been built yet.
    pub fn mentions_of(&self, entity_id: &str) -> Vec<String> {
        self.mentions
            .by_entity
            .get(entity_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Persist a fresh mentions list for `entity_id`. The future entity-
    /// extraction skill writes through this; #0008 ships it so the call site
    /// is stable when the skill lands.
    pub fn set_mentions(&mut self, entity_id: &str, refs: Vec<String>) -> Result<()> {
        if refs.is_empty() {
            self.mentions.by_entity.remove(entity_id);
        } else {
            self.mentions
                .by_entity
                .insert(entity_id.to_string(), refs);
        }
        save_mentions(&self.root, &self.mentions)
    }

    // ---- SceneRecord ------------------------------------------------------

    pub fn save_scene_record(&self, record: &SceneRecord) -> Result<()> {
        let dir = self.root.join("scene_records").join(&record.chapter);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", record.scene_id));
        write_json(&path, record)
    }

    pub fn load_scene_record(&self, chapter: &str, scene_id: &str) -> Option<SceneRecord> {
        let path = self
            .root
            .join("scene_records")
            .join(chapter)
            .join(format!("{scene_id}.json"));
        read_json(&path)
    }

    // ---- ChapterRollup ----------------------------------------------------

    #[allow(dead_code)] // #0011 wires this up
    pub fn save_chapter_rollup(&self, rollup: &ChapterRollup) -> Result<()> {
        let path = self
            .root
            .join("chapter_rollups")
            .join(format!("{}.json", rollup.chapter));
        write_json(&path, rollup)
    }

    #[allow(dead_code)] // #0011 wires this up
    pub fn load_chapter_rollup(&self, chapter: &str) -> Option<ChapterRollup> {
        let path = self
            .root
            .join("chapter_rollups")
            .join(format!("{chapter}.json"));
        read_json(&path)
    }
}

// ============================================================================
// JSON helpers (private)
// ============================================================================

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(value)?;
    std::fs::write(path, s)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn load_mentions(agent_root: &Path) -> Mentions {
    read_json::<Mentions>(&agent_root.join("mentions.json")).unwrap_or_default()
}

fn save_mentions(agent_root: &Path, mentions: &Mentions) -> Result<()> {
    write_json(&agent_root.join("mentions.json"), mentions)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tempdir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "ckwriter-agent-layer-{}-{nanos}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn meta_with(refs: &[&str], hash: &str, pinned: bool) -> AgentArtifactMeta {
        AgentArtifactMeta {
            schema: CURRENT_SCHEMA,
            source_hash: hash.to_string(),
            source_refs: refs.iter().map(|s| s.to_string()).collect(),
            created_at: 0,
            created_by_skill: "test_skill@1".into(),
            model_id: "test-model".into(),
            confidence: Confidence::Medium,
            pinned,
        }
    }

    fn pmeta(id: &str, hash: &str) -> ParagraphMeta {
        ParagraphMeta {
            id: id.into(),
            hash: hash.into(),
            locked: false,
        }
    }

    #[test]
    fn open_creates_agent_directory_tree_idempotently() {
        let root = tempdir();
        let layer = AgentLayer::open(&root).unwrap();
        assert!(layer.root.is_dir());
        assert!(layer.root.join("scene_records").is_dir());
        assert!(layer.root.join("chapter_rollups").is_dir());
        assert!(layer.root.join("summaries").is_dir());
        // Second open is a no-op — must not error or wipe contents.
        let _again = AgentLayer::open(&root).unwrap();
    }

    #[test]
    fn empty_mentions_round_trips_and_set_persists() {
        let root = tempdir();
        let mut layer = AgentLayer::open(&root).unwrap();

        // Empty default by `mentions_of`, no file required.
        assert!(layer.mentions_of("yara").is_empty());

        // set_mentions writes to disk.
        layer
            .set_mentions("yara", vec!["p_aaaaaaaa".into(), "p_bbbbbbbb".into()])
            .unwrap();
        assert!(layer.mentions_path().exists());

        // Reload from disk; refs survive.
        let layer2 = AgentLayer::open(&root).unwrap();
        assert_eq!(
            layer2.mentions_of("yara"),
            vec!["p_aaaaaaaa".to_string(), "p_bbbbbbbb".to_string()]
        );
    }

    #[test]
    fn set_mentions_with_empty_refs_clears_entry() {
        let root = tempdir();
        let mut layer = AgentLayer::open(&root).unwrap();
        layer.set_mentions("yara", vec!["p_aaaaaaaa".into()]).unwrap();
        layer.set_mentions("yara", vec![]).unwrap();
        let layer2 = AgentLayer::open(&root).unwrap();
        assert!(layer2.mentions_of("yara").is_empty());
    }

    #[test]
    fn compute_source_hash_changes_when_any_paragraph_hash_changes() {
        let refs = vec!["p_a".to_string(), "p_b".to_string()];
        let h1 =
            compute_source_hash(&refs, |id| match id {
                "p_a" => Some("aaaa".into()),
                "p_b" => Some("bbbb".into()),
                _ => None,
            })
            .unwrap();
        let h2 =
            compute_source_hash(&refs, |id| match id {
                "p_a" => Some("aaaa".into()),
                "p_b" => Some("CCCC".into()),
                _ => None,
            })
            .unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn compute_source_hash_returns_none_for_missing_paragraph() {
        let refs = vec!["p_a".to_string(), "p_gone".to_string()];
        let result = compute_source_hash(&refs, |id| match id {
            "p_a" => Some("aaaa".into()),
            _ => None,
        });
        assert!(result.is_none());
    }

    #[test]
    fn compute_source_hash_includes_id_so_swapped_ids_differ() {
        // Two paragraphs swap IDs but keep the same multiset of hashes.
        // Without `id:` in the domain, this would collide; with it, the
        // composite hash differs.
        let refs1 = vec!["p_a".to_string(), "p_b".to_string()];
        let refs2 = vec!["p_b".to_string(), "p_a".to_string()];
        let lookup = |id: &str| match id {
            "p_a" => Some("aaaa".into()),
            "p_b" => Some("bbbb".into()),
            _ => None,
        };
        let h1 = compute_source_hash(&refs1, lookup).unwrap();
        let h2 = compute_source_hash(&refs2, lookup).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn is_stale_truth_table() {
        let paragraphs = vec![pmeta("p_a", "aaaa"), pmeta("p_b", "bbbb")];
        let refs: Vec<String> = vec!["p_a".into(), "p_b".into()];
        let current = compute_source_hash(&refs, |id| {
            paragraphs.iter().find(|p| p.id == id).map(|p| p.hash.clone())
        })
        .unwrap();

        // Unchanged source → not stale.
        let m = meta_with(&["p_a", "p_b"], &current, false);
        assert!(!is_stale(&m, &paragraphs));

        // One paragraph hash changed → stale.
        let drifted = vec![pmeta("p_a", "aaaa"), pmeta("p_b", "CCCC")];
        assert!(is_stale(&m, &drifted));

        // A source paragraph was deleted → stale.
        let deleted = vec![pmeta("p_a", "aaaa")];
        assert!(is_stale(&m, &deleted));

        // Pinned beats every drift case.
        let pinned = meta_with(&["p_a", "p_b"], &current, true);
        assert!(!is_stale(&pinned, &drifted));
        assert!(!is_stale(&pinned, &deleted));
    }

    #[test]
    fn scene_record_round_trip() {
        let root = tempdir();
        let layer = AgentLayer::open(&root).unwrap();
        let record = SceneRecord {
            meta: AgentArtifactMeta::new("scene_transcode@1", "gemma4:27b"),
            scene_id: "scene_001".into(),
            chapter: "Modern/010_Awakening".into(),
            beats: vec![],
        };
        layer.save_scene_record(&record).unwrap();
        let loaded = layer
            .load_scene_record("Modern/010_Awakening", "scene_001")
            .expect("scene record persists");
        assert_eq!(loaded, record);
    }
}
