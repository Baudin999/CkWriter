use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    #[default]
    Character,
    Location,
    Event,
    Timeline,
}

impl EntityKind {
    pub fn folder(self) -> &'static str {
        match self {
            EntityKind::Character => "Characters",
            EntityKind::Location => "Locations",
            EntityKind::Event => "Events",
            EntityKind::Timeline => "Timeline",
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            EntityKind::Character => "Characters",
            EntityKind::Location => "Locations",
            EntityKind::Event => "Events",
            EntityKind::Timeline => "Timeline",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub kind: String,
    pub id: String,
}

/// One snapshot of a character's state at a given chapter, captured by the AI
/// progression run. Older entries are never rewritten — the timeline reads
/// like a diary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressionEntry {
    /// Chapter include-path the snapshot was taken from (e.g. "Ancient/003_Storm").
    pub chapter: String,
    #[serde(default)]
    pub tone: String,
    #[serde(default)]
    pub situation: String,
    #[serde(default)]
    pub voice_summary: String,
    #[serde(default)]
    pub notable_changes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    #[serde(default)]
    pub kind: EntityKind,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub age: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tone: String,
    #[serde(default)]
    pub voice_notes: String,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub progression: Vec<ProgressionEntry>,
    #[serde(default)]
    pub children: Vec<String>,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub free_text: String,
    /// Free-form for events/timeline
    #[serde(default)]
    pub when: String,
    #[serde(default)]
    pub participants: Vec<String>,
}

impl Entity {
    pub fn new(kind: EntityKind, id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            name: name.into(),
            aliases: Vec::new(),
            age: String::new(),
            role: String::new(),
            category: String::new(),
            tone: String::new(),
            voice_notes: String::new(),
            relations: Vec::new(),
            progression: Vec::new(),
            children: Vec::new(),
            first_seen: String::new(),
            tags: Vec::new(),
            free_text: String::new(),
            when: String::new(),
            participants: Vec::new(),
        }
    }

    pub fn match_terms(&self) -> Vec<String> {
        let mut v: Vec<String> = std::iter::once(self.name.clone())
            .chain(self.aliases.iter().cloned())
            .filter(|s| !s.trim().is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn file_path(&self, root: &Path) -> PathBuf {
        root.join("Info").join(self.kind.folder()).join(format!("{}.json", self.id))
    }
}

/// One side of a mirror operation: add or remove `(kind → self_id)` on a
/// target entity. Used by `mirror_diff` so the inspector can keep symmetric
/// relations in sync without the caller having to track state by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorOp {
    /// Ensure `target` has a `(kind → self_id)` relation, adding it if absent.
    Add { target: String, kind: String },
    /// Remove every `(kind → self_id)` relation from `target`.
    Remove { target: String, kind: String },
}

/// Compute the mirror operations needed when an entity's relations change.
///
/// `inverse` returns the inverse kind of a relation kind, or `None` for
/// kinds that shouldn't be mirrored (free-form user kinds, asymmetric kinds
/// like "loyal to"). Relations whose target is empty or matches `self_id`
/// are skipped.
pub fn mirror_diff<F>(
    prev: &[Relation],
    next: &[Relation],
    self_id: &str,
    inverse: F,
) -> Vec<MirrorOp>
where
    F: Fn(&str) -> Option<String>,
{
    use std::collections::HashSet;

    fn key(r: &Relation) -> (String, String) {
        (r.kind.trim().to_lowercase(), r.id.trim().to_lowercase())
    }

    let prev_set: HashSet<(String, String)> = prev.iter().map(key).collect();
    let next_set: HashSet<(String, String)> = next.iter().map(key).collect();

    let mut ops: Vec<MirrorOp> = Vec::new();
    let mut emit = |target: &str, inv: String, add: bool| {
        if target.is_empty() || target == self_id {
            return;
        }
        if add {
            ops.push(MirrorOp::Add {
                target: target.to_string(),
                kind: inv,
            });
        } else {
            ops.push(MirrorOp::Remove {
                target: target.to_string(),
                kind: inv,
            });
        }
    };

    for r in next {
        if prev_set.contains(&key(r)) {
            continue;
        }
        if let Some(inv) = inverse(&r.kind) {
            emit(&r.id, inv, true);
        }
    }
    for r in prev {
        if next_set.contains(&key(r)) {
            continue;
        }
        if let Some(inv) = inverse(&r.kind) {
            emit(&r.id, inv, false);
        }
    }
    ops
}

pub fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            for low in ch.to_lowercase() {
                s.push(low);
            }
            prev_dash = false;
        } else if !prev_dash && !s.is_empty() {
            s.push('-');
            prev_dash = true;
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("untitled");
    }
    s
}

#[derive(Default)]
pub struct Entities {
    pub by_id: BTreeMap<String, Entity>,
}

impl Entities {
    pub fn load(root: &Path) -> Self {
        let mut by_id = BTreeMap::new();
        for kind in [
            EntityKind::Character,
            EntityKind::Location,
            EntityKind::Event,
            EntityKind::Timeline,
        ] {
            let dir = root.join("Info").join(kind.folder());
            if !dir.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::read_to_string(&p) {
                    Ok(s) => match serde_json::from_str::<Entity>(&s) {
                        Ok(mut e) => {
                            // Force kind to match folder so a misplaced file doesn't lie.
                            e.kind = kind;
                            by_id.insert(e.id.clone(), e);
                        }
                        Err(err) => log::warn!("entity parse failed {}: {err}", p.display()),
                    },
                    Err(err) => log::warn!("entity read failed {}: {err}", p.display()),
                }
            }
        }
        Self { by_id }
    }

    pub fn get(&self, id: &str) -> Option<&Entity> {
        self.by_id.get(id)
    }

    pub fn by_kind(&self, kind: EntityKind) -> Vec<&Entity> {
        let mut v: Vec<&Entity> = self.by_id.values().filter(|e| e.kind == kind).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn save(&mut self, root: &Path, e: Entity) -> Result<()> {
        let p = e.file_path(root);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&e)?;
        std::fs::write(&p, json)?;
        self.by_id.insert(e.id.clone(), e);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(kind: &str, id: &str) -> Relation {
        Relation {
            kind: kind.into(),
            id: id.into(),
        }
    }

    fn fake_inverse(k: &str) -> Option<String> {
        match k.trim().to_lowercase().as_str() {
            "child of" => Some("parent of".into()),
            "parent of" => Some("child of".into()),
            _ => None,
        }
    }

    #[test]
    fn mirror_diff_emits_add_for_new_relation_with_inverse() {
        let ops = mirror_diff(&[], &[rel("child of", "yara")], "wua", fake_inverse);
        assert_eq!(
            ops,
            vec![MirrorOp::Add {
                target: "yara".into(),
                kind: "parent of".into(),
            }]
        );
    }

    #[test]
    fn mirror_diff_emits_remove_for_dropped_relation() {
        let ops = mirror_diff(&[rel("child of", "yara")], &[], "wua", fake_inverse);
        assert_eq!(
            ops,
            vec![MirrorOp::Remove {
                target: "yara".into(),
                kind: "parent of".into(),
            }]
        );
    }

    #[test]
    fn mirror_diff_skips_unchanged_and_unknown_kinds() {
        let prev = vec![rel("child of", "yara"), rel("haunted by", "ghost")];
        let next = vec![rel("child of", "yara"), rel("haunted by", "ghost")];
        let ops = mirror_diff(&prev, &next, "wua", fake_inverse);
        assert!(ops.is_empty());
    }

    #[test]
    fn mirror_diff_skips_self_relations_and_blank_targets() {
        let next = vec![rel("child of", "wua"), rel("child of", "")];
        let ops = mirror_diff(&[], &next, "wua", fake_inverse);
        assert!(ops.is_empty());
    }

    #[test]
    fn mirror_diff_is_case_insensitive_on_kind_and_id() {
        // "Child Of" → "yara" should match "child of" → "Yara" (no change).
        let prev = vec![rel("child of", "yara")];
        let next = vec![rel("Child Of", "Yara")];
        let ops = mirror_diff(&prev, &next, "wua", fake_inverse);
        assert!(ops.is_empty());
    }

    #[test]
    fn old_entity_json_loads_without_category_or_progression() {
        // Pre-Phase-1 JSON files exist on disk. If a future change drops
        // #[serde(default)] from these new fields, those files stop loading
        // and the writer's characters silently vanish — pin the contract.
        let raw = r#"{
            "id": "yara",
            "kind": "character",
            "name": "Yara",
            "aliases": ["Y."],
            "role": "sister"
        }"#;
        let e: Entity = serde_json::from_str(raw).expect("parse legacy");
        assert_eq!(e.name, "Yara");
        assert_eq!(e.category, "");
        assert!(e.progression.is_empty());
    }
}
