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
    pub tone: String,
    #[serde(default)]
    pub voice_notes: String,
    #[serde(default)]
    pub relations: Vec<Relation>,
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
            tone: String::new(),
            voice_notes: String::new(),
            relations: Vec::new(),
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
