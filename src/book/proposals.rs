//! Proposal queue — the Agent → Author promotion valve. See ticket #0008
//! §"Proposal queue".
//!
//! Skills *propose* canon changes; they don't apply them. The orchestrator
//! (#0009) hands skills a tool registry that excludes direct entity writes,
//! leaving `Proposals::queue` as the only path. Authors then review pending
//! proposals and `confirm` (apply to the Author Layer + mark resolved) or
//! `reject` (mark resolved with a reason).
//!
//! v1 stores everything in a single `Info/agent/proposals.json`. Resolved
//! proposals stay in the file for audit; an archival step can land later.
//!
//! **Trust boundary note (also in ticket #0008):** `Book::save_entity` is
//! *not* restricted at the type level in v1. The boundary is enforced at the
//! orchestrator layer (#0009). This module ships the storage and apply
//! semantics; the rule "skills can only write through `propose`" is policy,
//! not Rust.

// First non-test consumer is the orchestrator (#0009) and the inspector UI
// for review (later UI ticket). Unit tests exercise queue / confirm /
// reject / pending end-to-end. Allow dead_code at module scope rather than
// peppering attributes through every kind of `apply_*` helper.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::book::entity::{Entities, Entity, EntityKind, Relation};

/// Schema version for the proposals file. Bump on incompatible changes;
/// loader accepts `schema <= CURRENT_SCHEMA`.
const CURRENT_SCHEMA: u32 = 1;

/// Public alias so call sites read clearly.
pub type ProposalId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProposalKind {
    EntityCreate,
    EntityFieldUpdate,
    RelationAdd,
    RelationRemove,
}

/// Which entity the proposal targets. For `EntityCreate` this is the
/// intended ID of the new entity (must not already exist on apply); for the
/// rest, an existing entity ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalTarget {
    pub entity_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: ProposalId,
    pub kind: ProposalKind,
    pub target: ProposalTarget,
    /// Typed per `kind`, validated on `confirm`. See `apply_change`.
    pub change: serde_json::Value,
    pub reason: String,
    pub created_at: i64,
    pub created_by_skill: String,
    pub status: ProposalStatus,
    #[serde(default)]
    pub resolved_at: Option<i64>,
    /// Author-supplied note when rejecting. None for pending and most
    /// confirmed entries.
    #[serde(default)]
    pub resolution_note: Option<String>,
}

impl Proposal {
    pub fn new_pending(
        kind: ProposalKind,
        target: ProposalTarget,
        change: serde_json::Value,
        reason: impl Into<String>,
        skill: impl Into<String>,
    ) -> Self {
        Self {
            id: String::new(), // assigned by Proposals::queue
            kind,
            target,
            change,
            reason: reason.into(),
            created_at: now_unix(),
            created_by_skill: skill.into(),
            status: ProposalStatus::Pending,
            resolved_at: None,
            resolution_note: None,
        }
    }
}

/// Partial-fields patch shape for `EntityFieldUpdate.change`. Every field is
/// `Option<...>`; `Some(_)` means "set this", `None` means "leave alone".
/// The set is deliberately a subset of `Entity`'s fields — id and kind are
/// not patchable here (kind is invariant; id is the target itself). New
/// fields land additively with `#[serde(default)]`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub age: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tone: Option<String>,
    #[serde(default)]
    pub voice_notes: Option<String>,
    #[serde(default)]
    pub first_seen: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub free_text: Option<String>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub participants: Option<Vec<String>>,
}

impl EntityPatch {
    pub fn merge_into(self, e: &mut Entity) {
        if let Some(v) = self.name {
            e.name = v;
        }
        if let Some(v) = self.aliases {
            e.aliases = v;
        }
        if let Some(v) = self.age {
            e.age = v;
        }
        if let Some(v) = self.role {
            e.role = v;
        }
        if let Some(v) = self.category {
            e.category = v;
        }
        if let Some(v) = self.tone {
            e.tone = v;
        }
        if let Some(v) = self.voice_notes {
            e.voice_notes = v;
        }
        if let Some(v) = self.first_seen {
            e.first_seen = v;
        }
        if let Some(v) = self.tags {
            e.tags = v;
        }
        if let Some(v) = self.free_text {
            e.free_text = v;
        }
        if let Some(v) = self.when {
            e.when = v;
        }
        if let Some(v) = self.participants {
            e.participants = v;
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProposalsFile {
    #[serde(default = "default_schema")]
    schema: u32,
    #[serde(default)]
    proposals: Vec<Proposal>,
}

fn default_schema() -> u32 {
    CURRENT_SCHEMA
}

#[derive(Default)]
pub struct Proposals {
    file_path: PathBuf,
    proposals: Vec<Proposal>,
    /// Last microsecond used for ID minting. The collision-bump path nudges
    /// this forward so two queues in the same microsecond still get
    /// monotonic IDs.
    last_micros: u128,
}

impl Proposals {
    /// Open the proposal queue for a book. Reads `Info/agent/proposals.json`
    /// if it exists; otherwise starts empty (no file is written until the
    /// first queue/confirm/reject).
    pub fn open(book_root: &Path) -> Self {
        let file_path = book_root.join("Info").join("agent").join("proposals.json");
        let proposals = match std::fs::read_to_string(&file_path) {
            Ok(s) => serde_json::from_str::<ProposalsFile>(&s)
                .map(|f| f.proposals)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        Self {
            file_path,
            proposals,
            last_micros: 0,
        }
    }

    /// Queue a fresh proposal. Mints a sortable monotonic ID
    /// (`prop_{micros:030}`); see ticket #0008 status notes.
    pub fn queue(&mut self, mut p: Proposal) -> Result<ProposalId> {
        let id = self.mint_id();
        p.id = id.clone();
        p.status = ProposalStatus::Pending;
        p.resolved_at = None;
        self.proposals.push(p);
        self.persist()?;
        Ok(id)
    }

    pub fn pending(&self) -> Vec<&Proposal> {
        self.proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Pending)
            .collect()
    }

    /// Mark a proposal rejected with a free-form note. Idempotent on
    /// already-resolved proposals (returns Err if confirmed; updates note
    /// if already rejected).
    pub fn reject(&mut self, id: &str, reason: &str) -> Result<()> {
        let p = self
            .proposals
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("proposal not found: {id}"))?;
        if p.status == ProposalStatus::Confirmed {
            return Err(anyhow!("proposal already confirmed: {id}"));
        }
        p.status = ProposalStatus::Rejected;
        p.resolved_at = Some(now_unix());
        p.resolution_note = Some(reason.to_string());
        self.persist()
    }

    /// Apply a pending proposal to the Author Layer and mark it confirmed.
    ///
    /// Takes `&mut Entities` rather than `&mut Book` so it can be called
    /// from a Book method that already mut-borrows other Book fields. See
    /// `Book::confirm_proposal` for the wrapper.
    pub fn confirm(
        &mut self,
        id: &str,
        entities: &mut Entities,
        book_root: &Path,
    ) -> Result<()> {
        let idx = self
            .proposals
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| anyhow!("proposal not found: {id}"))?;
        if self.proposals[idx].status != ProposalStatus::Pending {
            return Err(anyhow!("proposal not pending: {id}"));
        }
        // Clone so apply_change doesn't double-borrow self.proposals.
        let proposal = self.proposals[idx].clone();
        apply_change(&proposal, entities, book_root)?;
        let p = &mut self.proposals[idx];
        p.status = ProposalStatus::Confirmed;
        p.resolved_at = Some(now_unix());
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = ProposalsFile {
            schema: CURRENT_SCHEMA,
            proposals: self.proposals.clone(),
        };
        let s = serde_json::to_string_pretty(&file)?;
        std::fs::write(&self.file_path, s)?;
        Ok(())
    }

    fn mint_id(&mut self) -> ProposalId {
        let now = now_micros();
        // Bump on collision so two queues in the same microsecond still
        // produce monotonically increasing IDs (status notes).
        let micros = if now > self.last_micros {
            now
        } else {
            self.last_micros + 1
        };
        self.last_micros = micros;
        format!("prop_{micros:030}")
    }
}

// ============================================================================
// Apply semantics — typed per ProposalKind, validated on confirm.
// ============================================================================

fn apply_change(p: &Proposal, entities: &mut Entities, book_root: &Path) -> Result<()> {
    match p.kind {
        ProposalKind::EntityCreate => apply_create(p, entities, book_root),
        ProposalKind::EntityFieldUpdate => apply_field_update(p, entities, book_root),
        ProposalKind::RelationAdd => apply_relation_add(p, entities, book_root),
        ProposalKind::RelationRemove => apply_relation_remove(p, entities, book_root),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CreatePayload {
    kind: EntityKind,
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
}

fn apply_create(p: &Proposal, entities: &mut Entities, book_root: &Path) -> Result<()> {
    if entities.get(&p.target.entity_id).is_some() {
        return Err(anyhow!(
            "EntityCreate target already exists: {}",
            p.target.entity_id
        ));
    }
    let payload: CreatePayload = serde_json::from_value(p.change.clone())
        .map_err(|e| anyhow!("EntityCreate change invalid: {e}"))?;
    let mut entity = Entity::new(payload.kind, p.target.entity_id.clone(), payload.name);
    entity.aliases = payload.aliases;
    entities.save(book_root, entity)?;
    Ok(())
}

fn apply_field_update(p: &Proposal, entities: &mut Entities, book_root: &Path) -> Result<()> {
    let patch: EntityPatch = serde_json::from_value(p.change.clone())
        .map_err(|e| anyhow!("EntityFieldUpdate change invalid: {e}"))?;
    let mut entity = entities
        .get(&p.target.entity_id)
        .cloned()
        .ok_or_else(|| anyhow!("EntityFieldUpdate target missing: {}", p.target.entity_id))?;
    patch.merge_into(&mut entity);
    entities.save(book_root, entity)?;
    Ok(())
}

fn apply_relation_add(p: &Proposal, entities: &mut Entities, book_root: &Path) -> Result<()> {
    let rel: Relation = serde_json::from_value(p.change.clone())
        .map_err(|e| anyhow!("RelationAdd change invalid: {e}"))?;
    let mut entity = entities
        .get(&p.target.entity_id)
        .cloned()
        .ok_or_else(|| anyhow!("RelationAdd target missing: {}", p.target.entity_id))?;
    if !entity
        .relations
        .iter()
        .any(|r| r.kind.eq_ignore_ascii_case(&rel.kind) && r.id == rel.id)
    {
        entity.relations.push(rel);
    }
    entities.save(book_root, entity)?;
    Ok(())
}

fn apply_relation_remove(p: &Proposal, entities: &mut Entities, book_root: &Path) -> Result<()> {
    let rel: Relation = serde_json::from_value(p.change.clone())
        .map_err(|e| anyhow!("RelationRemove change invalid: {e}"))?;
    let mut entity = entities
        .get(&p.target.entity_id)
        .cloned()
        .ok_or_else(|| anyhow!("RelationRemove target missing: {}", p.target.entity_id))?;
    entity
        .relations
        .retain(|r| !(r.kind.eq_ignore_ascii_case(&rel.kind) && r.id == rel.id));
    entities.save(book_root, entity)?;
    Ok(())
}

// ============================================================================
// Time helpers
// ============================================================================

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_micros() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
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
            "ckwriter-proposals-{}-{nanos}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        // Also make Info/agent/ + Info/Characters/ since the apply tests
        // round-trip through Entities.
        std::fs::create_dir_all(p.join("Info/agent")).unwrap();
        std::fs::create_dir_all(p.join("Info/Characters")).unwrap();
        p
    }

    fn mint_proposal(kind: ProposalKind, target: &str, change: serde_json::Value) -> Proposal {
        Proposal::new_pending(
            kind,
            ProposalTarget {
                entity_id: target.into(),
            },
            change,
            "test reason",
            "test_skill@1",
        )
    }

    #[test]
    fn queue_assigns_monotonic_ids_and_persists() {
        let root = tempdir();
        let mut q = Proposals::open(&root);
        let id_a = q
            .queue(mint_proposal(
                ProposalKind::EntityCreate,
                "yara",
                serde_json::json!({"kind": "character", "name": "Yara"}),
            ))
            .unwrap();
        let id_b = q
            .queue(mint_proposal(
                ProposalKind::EntityCreate,
                "kal",
                serde_json::json!({"kind": "character", "name": "Kal"}),
            ))
            .unwrap();
        assert!(id_a < id_b, "ids must be monotonically sortable");

        // Reload; both pending.
        let q2 = Proposals::open(&root);
        assert_eq!(q2.pending().len(), 2);
    }

    #[test]
    fn confirm_entity_create_writes_canon_and_resolves() {
        let root = tempdir();
        let mut entities = Entities::load(&root, &[String::from("Info/Characters")]);
        let mut q = Proposals::open(&root);
        let id = q
            .queue(mint_proposal(
                ProposalKind::EntityCreate,
                "yara",
                serde_json::json!({
                    "kind": "character",
                    "name": "Yara",
                    "aliases": ["Y."],
                }),
            ))
            .unwrap();
        q.confirm(&id, &mut entities, &root).unwrap();

        // Entity now exists in the Author Layer.
        let saved = entities.get("yara").expect("yara saved");
        assert_eq!(saved.name, "Yara");
        assert_eq!(saved.aliases, vec!["Y.".to_string()]);
        assert!(root.join("Info/Characters/yara.json").exists());

        // Proposal is no longer pending; it's resolved.
        assert_eq!(q.pending().len(), 0);
    }

    #[test]
    fn confirm_field_update_merges_partial_patch() {
        let root = tempdir();
        let mut entities = Entities::load(&root, &[String::from("Info/Characters")]);
        let mut yara = Entity::new(EntityKind::Character, "yara", "Yara");
        yara.role = "sister".into();
        yara.tone = "bright".into();
        entities.save(&root, yara).unwrap();

        let mut q = Proposals::open(&root);
        // Patch overrides role; leaves tone alone (None ≡ "leave").
        let id = q
            .queue(mint_proposal(
                ProposalKind::EntityFieldUpdate,
                "yara",
                serde_json::json!({"role": "elder"}),
            ))
            .unwrap();
        q.confirm(&id, &mut entities, &root).unwrap();

        let saved = entities.get("yara").unwrap();
        assert_eq!(saved.role, "elder");
        assert_eq!(saved.tone, "bright", "untouched fields preserved");
    }

    #[test]
    fn confirm_relation_add_is_idempotent_on_duplicate() {
        let root = tempdir();
        let mut entities = Entities::load(&root, &[String::from("Info/Characters")]);
        let mut yara = Entity::new(EntityKind::Character, "yara", "Yara");
        yara.relations.push(Relation {
            kind: "child of".into(),
            id: "wua".into(),
        });
        entities.save(&root, yara).unwrap();

        let mut q = Proposals::open(&root);
        let id = q
            .queue(mint_proposal(
                ProposalKind::RelationAdd,
                "yara",
                serde_json::json!({"kind": "Child Of", "id": "wua"}),
            ))
            .unwrap();
        q.confirm(&id, &mut entities, &root).unwrap();

        let saved = entities.get("yara").unwrap();
        assert_eq!(
            saved.relations.len(),
            1,
            "duplicate relation must not be re-added"
        );
    }

    #[test]
    fn confirm_relation_remove_drops_matching_kind_and_id() {
        let root = tempdir();
        let mut entities = Entities::load(&root, &[String::from("Info/Characters")]);
        let mut yara = Entity::new(EntityKind::Character, "yara", "Yara");
        yara.relations.push(Relation {
            kind: "child of".into(),
            id: "wua".into(),
        });
        yara.relations.push(Relation {
            kind: "ally".into(),
            id: "kal".into(),
        });
        entities.save(&root, yara).unwrap();

        let mut q = Proposals::open(&root);
        let id = q
            .queue(mint_proposal(
                ProposalKind::RelationRemove,
                "yara",
                serde_json::json!({"kind": "child of", "id": "wua"}),
            ))
            .unwrap();
        q.confirm(&id, &mut entities, &root).unwrap();

        let saved = entities.get("yara").unwrap();
        assert_eq!(saved.relations.len(), 1);
        assert_eq!(saved.relations[0].kind, "ally");
    }

    #[test]
    fn reject_marks_resolved_with_note() {
        let root = tempdir();
        let mut q = Proposals::open(&root);
        let id = q
            .queue(mint_proposal(
                ProposalKind::EntityCreate,
                "ghost",
                serde_json::json!({"kind": "character", "name": "Ghost"}),
            ))
            .unwrap();
        q.reject(&id, "model invented this character").unwrap();

        let q2 = Proposals::open(&root);
        assert!(q2.pending().is_empty());
        // Find the rejected entry directly.
        let entry = q2.proposals.iter().find(|p| p.id == id).unwrap();
        assert_eq!(entry.status, ProposalStatus::Rejected);
        assert_eq!(
            entry.resolution_note.as_deref(),
            Some("model invented this character")
        );
    }

    #[test]
    fn confirm_twice_errors() {
        let root = tempdir();
        let mut entities = Entities::load(&root, &[String::from("Info/Characters")]);
        let mut q = Proposals::open(&root);
        let id = q
            .queue(mint_proposal(
                ProposalKind::EntityCreate,
                "yara",
                serde_json::json!({"kind": "character", "name": "Yara"}),
            ))
            .unwrap();
        q.confirm(&id, &mut entities, &root).unwrap();
        let err = q.confirm(&id, &mut entities, &root).unwrap_err();
        assert!(err.to_string().contains("not pending"));
    }
}
