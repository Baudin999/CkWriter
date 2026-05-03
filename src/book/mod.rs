pub mod chapters;
pub mod data;
pub mod dismissals;
pub mod entity;
pub mod latex;
pub mod manuscript;
pub mod tree;

use anyhow::{anyhow, Result};
use data::BookData;
use dismissals::Dismissals;
use entity::{Entities, Entity, EntityKind};
use manuscript::Manuscript;
use std::path::{Path, PathBuf};
use tree::FileNode;

#[derive(Debug, Clone)]
pub struct Chapter {
    /// Managed folder this chapter lives in (e.g. `Ancient`, `Modern`).
    /// Empty for chapters discovered outside `manuscript::MANAGED_FOLDERS`.
    pub folder: String,
    /// CamelCase identifier matching the `name` in `manuscript.json` and the
    /// suffix of the on-disk filename. Stable across renumbering — DnD
    /// changes positions, not names.
    pub name: String,
    #[allow(dead_code)]
    pub include_path: String,
    pub file_path: PathBuf,
    pub display_title: String,
    pub in_manuscript: bool,
}

pub struct Book {
    pub root: PathBuf,
    #[allow(dead_code)]
    pub main_tex: PathBuf,
    pub chapters: Vec<Chapter>,
    pub manuscript: Manuscript,
    pub file_tree: FileNode,
    pub entities: Entities,
    pub voice_prompt: String,
    pub roadmap: String,
    pub config: BookConfig,
    // Read by the inspector / scope panel in Phase 2 (category dropdown);
    // see book/data.rs for shape.
    #[allow(dead_code)]
    pub data: BookData,
    pub dismissals: Dismissals,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BookConfig {
    #[serde(default = "default_main")]
    pub main_tex: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_voice_prompt")]
    pub voice_prompt_file: String,
    #[serde(default = "default_roadmap")]
    pub roadmap_file: String,
}

fn default_main() -> String {
    "main.tex".into()
}
fn default_voice_prompt() -> String {
    "Info/Writing Assistant/voice-system-prompt.md".into()
}
fn default_roadmap() -> String {
    "Info/World Building/Plot.txt".into()
}

impl Default for BookConfig {
    fn default() -> Self {
        Self {
            main_tex: default_main(),
            model: None,
            voice_prompt_file: default_voice_prompt(),
            roadmap_file: default_roadmap(),
        }
    }
}

impl Book {
    pub fn open(root: &Path) -> Result<Self> {
        if !root.exists() {
            return Err(anyhow!("book root does not exist: {}", root.display()));
        }
        let config = load_config(root);
        let main_tex = root.join(&config.main_tex);
        if !main_tex.exists() {
            return Err(anyhow!("main TeX file not found: {}", main_tex.display()));
        }

        let manuscript = load_or_seed_manuscript(root, &main_tex)?;
        // Reconcile filenames + main.tex against the manuscript. Idempotent —
        // does nothing on subsequent opens unless something drifted.
        chapters::commit(root, &config.main_tex, &manuscript)?;

        let chapters_list = build_chapters(root, &manuscript);

        let entities = Entities::load(root);
        let data = BookData::load(root);
        let dismissals = Dismissals::load(root);

        let voice_prompt =
            std::fs::read_to_string(root.join(&config.voice_prompt_file)).unwrap_or_default();
        let roadmap = std::fs::read_to_string(root.join(&config.roadmap_file)).unwrap_or_default();

        let file_tree = FileNode::build(root);

        Ok(Self {
            root: root.to_path_buf(),
            main_tex,
            chapters: chapters_list,
            manuscript,
            file_tree,
            entities,
            voice_prompt,
            roadmap,
            config,
            data,
            dismissals,
        })
    }

    pub fn title(&self) -> &str {
        self.root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Book")
    }

    pub fn chapter_by_path(&self, path: &Path) -> Option<&Chapter> {
        self.chapters.iter().find(|c| c.file_path == path)
    }

    /// Re-derive `chapters` and `file_tree` from disk + `manuscript` after a
    /// chapter operation. Cheaper than re-running `Book::open` because it
    /// skips entity/voice/roadmap reloads that aren't affected.
    pub fn reload_chapters(&mut self) {
        self.chapters = build_chapters(&self.root, &self.manuscript);
        self.file_tree = FileNode::build(&self.root);
    }

    pub fn entity(&self, id: &str) -> Option<&Entity> {
        self.entities.get(id)
    }

    pub fn save_entity(&mut self, e: Entity) -> Result<()> {
        self.entities.save(&self.root, e)
    }

    // Called from the settings dialog in Phase 3 once the categories /
    // relation-kinds editor lands; phase 1 only persists if the writer ever
    // edits book.json by hand.
    #[allow(dead_code)]
    pub fn save_data(&self) -> Result<()> {
        self.data.save(&self.root)
    }

    pub fn entities_of(&self, kind: EntityKind) -> Vec<&Entity> {
        self.entities.by_kind(kind)
    }

    pub fn reload_entities(&mut self) {
        self.entities = Entities::load(&self.root);
    }
}

fn load_config(root: &Path) -> BookConfig {
    let p = root.join("Info/index.json");
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => BookConfig::default(),
    }
}

fn read_chapter_title(path: &Path) -> Option<String> {
    let txt = std::fs::read_to_string(path).ok()?;
    latex::extract_chapter_title(&txt)
}

/// Either load `Info/manuscript.json` or, on first use, derive it from the
/// `\include{}` lines already in `main.tex`. Only paths shaped like
/// `<managed-folder>/<NNN>_<name>` are migrated; anything else is left for
/// the writer to handle by hand.
fn load_or_seed_manuscript(root: &Path, main_tex: &Path) -> Result<Manuscript> {
    let path = manuscript::file_path(root);
    if path.exists() {
        return Ok(Manuscript::load(root));
    }
    let main_text = std::fs::read_to_string(main_tex)?;
    let mut chapters: Vec<manuscript::ChapterRef> = Vec::new();
    for inc in latex::parse_includes(&main_text) {
        let Some((folder, stem)) = inc.split_once('/') else {
            log::info!("seed_manuscript: skipping non-folder include {inc:?}");
            continue;
        };
        if !manuscript::MANAGED_FOLDERS.contains(&folder) {
            log::info!("seed_manuscript: skipping unmanaged folder include {inc:?}");
            continue;
        }
        let name = manuscript::strip_number_prefix(stem).to_string();
        chapters.push(manuscript::ChapterRef {
            folder: folder.to_string(),
            name,
        });
    }
    let m = Manuscript { chapters };
    m.save(root)?;
    Ok(m)
}

/// Build the in-memory chapter list: every manuscript entry first (with
/// derived numbered file paths), then orphans (files in managed folders that
/// aren't in the manuscript).
fn build_chapters(root: &Path, manuscript: &Manuscript) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::with_capacity(manuscript.chapters.len());
    let mut per_folder_pos: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for c in &manuscript.chapters {
        let pos = per_folder_pos.entry(c.folder.as_str()).or_insert(0);
        let include_path = manuscript::derive_include_path(*pos, &c.folder, &c.name);
        let file_path = root.join(format!("{include_path}.tex"));
        let display_title = read_chapter_title(&file_path).unwrap_or_else(|| {
            // Fall back to a humanized name so the sidebar always shows
            // something readable, even before the writer adds \chapter{...}.
            manuscript::humanize(&c.name)
        });
        out.push(Chapter {
            folder: c.folder.clone(),
            name: c.name.clone(),
            include_path,
            file_path,
            display_title,
            in_manuscript: true,
        });
        *pos += 1;
    }

    let listings = chapters::list_folders(root, manuscript);
    let mut orphan_keys: Vec<&String> = listings.keys().collect();
    orphan_keys.sort();
    for folder in orphan_keys {
        for orphan in &listings[folder].orphans {
            let display_title = read_chapter_title(&orphan.file_path)
                .unwrap_or_else(|| manuscript::humanize(&orphan.name));
            out.push(Chapter {
                folder: folder.clone(),
                name: orphan.name.clone(),
                include_path: format!("{folder}/{}", orphan.name),
                file_path: orphan.file_path.clone(),
                display_title,
                in_manuscript: false,
            });
        }
    }
    out
}
