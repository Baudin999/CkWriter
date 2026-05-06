pub mod accessors;
pub mod agent_layer;
pub mod chapter_meta;
pub mod chapters;
pub mod data;
pub mod dismissals;
pub mod entity;
pub mod init;
pub mod latex;
pub mod manuscript;
pub mod paragraphs;
pub mod proposals;
pub mod suggestions;
pub mod tree;

use accessors::Accessors;
use agent_layer::AgentLayer;
use anyhow::{anyhow, Result};
use chapter_meta::ChapterMeta;
use data::BookData;
use entity::{Entities, Entity, EntityKind};
use manuscript::Manuscript;
use proposals::Proposals;
use std::path::{Path, PathBuf};
use suggestions::SuggestionStore;
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
    /// Per-chapter metadata — summary, goals, word count, last voice score.
    /// Loaded from `Info/chapters/<folder>/<name>.json` in `build_chapters`;
    /// the on-disk file is the source of truth, this is the cached copy.
    pub meta: ChapterMeta,
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
    /// Per-chapter suggestion lifecycle, lazy-loaded from
    /// `Info/suggestions/<folder>/<name>.json`. Replaces the old
    /// `coach-dismissals.json` (migrated on first open).
    pub suggestions: SuggestionStore,
    /// Agent Layer storage — model-derived artifacts under `Info/agent/`.
    /// See ticket #0008. The skill framework that produces artifacts is
    /// #0009; v1 ships storage primitives so later tickets can light up
    /// without schema migrations.
    // First reader is the orchestrator (#0009); the field is wired up here
    // so #0009 doesn't need to thread it back into Book.
    #[allow(dead_code)]
    pub agent_layer: AgentLayer,
    /// Pending + resolved Agent → Author proposals. The trust boundary
    /// (skills can only mutate canon through `propose`) is enforced at the
    /// orchestrator (#0009), not here.
    // Same as `agent_layer`: ships ahead of its first non-test caller.
    #[allow(dead_code)]
    pub proposals: Proposals,
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
    /// Folders under the book root whose `.tex` files CkWriter manages — i.e.
    /// the chapter folders (#0033). Was hardcoded as
    /// `manuscript::MANAGED_FOLDERS = ["Ancient","Modern"]`; the default
    /// here matches that so existing books keep working when `Info/index.json`
    /// has no `manuscript_folders` key. Fresh projects from `init_project`
    /// write `[]` and let the writer add folders via project settings.
    #[serde(default = "manuscript::default_managed_folders")]
    pub manuscript_folders: Vec<String>,
    /// Folders relative to the book root that hold character entity JSONs
    /// (#0033). The first entry is the canonical save location for new
    /// characters; every entry contributes to the in-memory entity store on
    /// `Book::open`. Locations / Events / Timeline kinds keep their hardcoded
    /// folders for v1.
    #[serde(default = "default_authors_folders")]
    pub authors_folders: Vec<String>,
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
fn default_authors_folders() -> Vec<String> {
    vec!["Info/Characters".into()]
}

impl Default for BookConfig {
    fn default() -> Self {
        Self {
            main_tex: default_main(),
            model: None,
            voice_prompt_file: default_voice_prompt(),
            roadmap_file: default_roadmap(),
            manuscript_folders: manuscript::default_managed_folders(),
            authors_folders: default_authors_folders(),
        }
    }
}

impl BookConfig {
    // Wired up by the project-folder editor in the same ticket (#0033 step 6);
    // pre-existing call sites only ever read `Info/index.json` so nothing else
    // exercises this yet.
    #[allow(dead_code)]
    pub fn save(&self, root: &Path) -> Result<()> {
        let p = root.join("Info/index.json");
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(&p, s)?;
        Ok(())
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

        let manuscript = load_or_seed_manuscript(root, &main_tex, &config.manuscript_folders)?;
        // Reconcile filenames + main.tex against the manuscript. Idempotent —
        // does nothing on subsequent opens unless something drifted.
        chapters::commit(
            root,
            &config.main_tex,
            &manuscript,
            &config.manuscript_folders,
        )?;

        let chapters_list = build_chapters(root, &manuscript, &config.manuscript_folders);

        // Idempotent folder bootstrap for the new Author Layer kinds (#0008)
        // and the Agent Layer root. Existing books just see mkdir -p no-ops.
        for kind_folder in ["Factions", "Items", "MagicRules"] {
            let _ = std::fs::create_dir_all(root.join("Info").join(kind_folder));
        }
        let agent_layer = AgentLayer::open(root)?;
        let proposals = Proposals::open(root);

        let entities = Entities::load(root, &config.authors_folders);
        let data = BookData::load(root);

        // Migrate the legacy `Info/coach-dismissals.json` to the per-chapter
        // suggestion store. Idempotent — the rename to `.migrated` ensures we
        // run at most once per book.
        if let Err(e) = suggestions_migrate::run(root, &chapters_list) {
            log::warn!("legacy dismissals migration failed: {e}");
        }
        let suggestions = SuggestionStore::default();

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
            suggestions,
            agent_layer,
            proposals,
        })
    }

    /// Typed read-only view over the Author Layer (`characters`, `locations`,
    /// `factions`, `items`, `magic_rules`, `timeline`). Mutation goes through
    /// `Book::save_entity` (direct author edits) or
    /// `Book::confirm_proposal` (agent-originated changes).
    // First non-test caller is the orchestrator (#0009); shipping the entry
    // point here so the orchestrator borrows from Book directly.
    #[allow(dead_code)]
    pub fn accessors(&self) -> Accessors<'_> {
        Accessors::new(self)
    }

    /// Apply a pending proposal: writes to the Author Layer, marks the
    /// proposal confirmed, and persists. Wraps `Proposals::confirm` so the
    /// double-borrow on `&mut Book` doesn't surface to callers.
    #[allow(dead_code)] // wired up by the inspector UI in a later ticket
    pub fn confirm_proposal(&mut self, id: &str) -> Result<()> {
        self.proposals.confirm(id, &mut self.entities, &self.root)
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
        self.chapters =
            build_chapters(&self.root, &self.manuscript, &self.config.manuscript_folders);
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
        self.entities = Entities::load(&self.root, &self.config.authors_folders);
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
fn load_or_seed_manuscript(
    root: &Path,
    main_tex: &Path,
    folders: &[String],
) -> Result<Manuscript> {
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
        if !folders.iter().any(|f| f == folder) {
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
fn build_chapters(root: &Path, manuscript: &Manuscript, folders: &[String]) -> Vec<Chapter> {
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
        let meta = load_or_seed_meta(root, &c.folder, &c.name, &file_path);
        out.push(Chapter {
            folder: c.folder.clone(),
            name: c.name.clone(),
            include_path,
            file_path,
            display_title,
            in_manuscript: true,
            meta,
        });
        *pos += 1;
    }

    let listings = chapters::list_folders(root, manuscript, folders);
    let mut orphan_keys: Vec<&String> = listings.keys().collect();
    orphan_keys.sort();
    for folder in orphan_keys {
        for orphan in &listings[folder].orphans {
            let display_title = read_chapter_title(&orphan.file_path)
                .unwrap_or_else(|| manuscript::humanize(&orphan.name));
            let meta = load_or_seed_meta(root, folder, &orphan.name, &orphan.file_path);
            out.push(Chapter {
                folder: folder.clone(),
                name: orphan.name.clone(),
                include_path: format!("{folder}/{}", orphan.name),
                file_path: orphan.file_path.clone(),
                display_title,
                in_manuscript: false,
                meta,
            });
        }
    }
    out
}

/// Load a chapter's sidecar metadata from disk, or — on first open — seed it
/// from the chapter's prose (for `word_count`) and the legacy `.notes.md`
/// scratchpad (for `plot_notes`). The seeded file is written back so future
/// opens are pure reads.
fn load_or_seed_meta(root: &Path, folder: &str, name: &str, tex_path: &Path) -> ChapterMeta {
    let meta_path = chapter_meta::file_path(root, folder, name);
    if meta_path.exists() {
        return chapter_meta::load(root, folder, name);
    }
    let mut meta = ChapterMeta::default();
    if let Ok(tex) = std::fs::read_to_string(tex_path) {
        let prose = latex::to_prose(&tex);
        meta.word_count = chapter_meta::word_count_from_prose(&prose);
    }
    if let Some(notes) = read_legacy_notes(tex_path) {
        meta.plot_notes = notes;
    }
    if let Err(e) = chapter_meta::save(root, folder, name, &meta) {
        log::warn!("seed chapter meta {folder}/{name} failed: {e}");
    }
    meta
}

/// One-shot migrator from `Info/coach-dismissals.json` → per-chapter
/// suggestion files. Idempotency marker is the rename of the legacy file to
/// `.migrated`; missing legacy file is also a no-op.
mod suggestions_migrate {
    use super::dismissals::{normalize, LEGACY_FILE_NAME};
    use super::paragraphs::parse_and_match;
    use super::suggestions::{id_hash, ChapterSuggestions, Status, SuggestionRecord};
    use super::Chapter;
    use anyhow::Result;
    use serde::Deserialize;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    /// Legacy on-disk shape: `chapter_name -> pipeline_label -> set<normalized_quote>`.
    /// Same as the old `Dismissals::by_chapter`; we keep a private shadow here
    /// so the active code can drop the `Dismissals` type entirely.
    #[derive(Debug, Default, Deserialize)]
    struct Legacy {
        #[serde(default)]
        by_chapter: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    }

    pub fn run(root: &Path, chapters: &[Chapter]) -> Result<()> {
        let legacy_path = root.join(LEGACY_FILE_NAME);
        if !legacy_path.exists() {
            return Ok(());
        }
        let raw = std::fs::read_to_string(&legacy_path)?;
        let legacy: Legacy = serde_json::from_str(&raw).unwrap_or_default();
        let now = now_unix();

        let mut per_chapter: BTreeMap<(String, String), ChapterSuggestions> = BTreeMap::new();
        for (chapter_name, by_pipeline) in &legacy.by_chapter {
            // Legacy keyed by stable CamelCase `name`; folder is implicit.
            // Look it up in the chapter list. If a chapter name appears in
            // multiple folders (rare; the writer would have to deliberately
            // duplicate), pick the first match — the old code didn't disambiguate
            // either, so the legacy data is already ambiguous.
            let chapter = chapters.iter().find(|c| &c.name == chapter_name);
            let (folder, name) = match chapter {
                Some(c) => (c.folder.clone(), c.name.clone()),
                None => {
                    log::warn!(
                        "migrate: chapter {chapter_name:?} not found in book; skipping"
                    );
                    continue;
                }
            };
            // Parse paragraphs from the chapter's .tex so we can resolve
            // paragraph_id for as many quotes as possible. Empty prior is fine
            // — paragraph ids are deterministic from normalized text + position.
            let tex_path = chapter.map(|c| c.file_path.clone());
            let para_index = tex_path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|text| (parse_and_match(&text, &[]), text));

            for (pipeline, quotes) in by_pipeline {
                for q in quotes {
                    let normalized = normalize(q);
                    if normalized.is_empty() {
                        continue;
                    }
                    let paragraph_id = match &para_index {
                        Some((paragraphs, text)) => paragraphs
                            .iter()
                            .find(|p| {
                                let (s, e) = p.char_range;
                                let body = text.get(s..e).unwrap_or("");
                                normalize(body).contains(&normalized)
                            })
                            .map(|p| p.id.clone()),
                        None => None,
                    };
                    let id = id_hash(pipeline, paragraph_id.as_deref(), &normalized);
                    let entry = per_chapter
                        .entry((folder.clone(), name.clone()))
                        .or_default();
                    entry.records.entry(id.clone()).or_insert(SuggestionRecord {
                        id,
                        pipeline: pipeline.clone(),
                        kind: String::new(),
                        paragraph_id,
                        // Legacy only persisted normalized quotes — that's all
                        // we have. Re-anchoring on hydrate has to use this as
                        // the raw quote too.
                        quote: q.clone(),
                        normalized_quote: normalized,
                        why: String::new(),
                        suggestion: String::new(),
                        status: Status::Dismissed,
                        created_at: now,
                        resolved_at: Some(now),
                        dismissal_note: None,
                    });
                }
            }
        }

        for ((folder, name), chapter) in &per_chapter {
            chapter.save(root, folder, name)?;
        }

        let mut migrated_path = legacy_path.clone();
        migrated_path.set_extension("json.migrated");
        std::fs::rename(&legacy_path, &migrated_path)?;
        log::info!(
            "migrate: ported {} chapter(s) of legacy dismissals; renamed {} → {}",
            per_chapter.len(),
            legacy_path.display(),
            migrated_path.display(),
        );
        Ok(())
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Read the legacy `<chapter>.notes.md` scratchpad that lived next to the
/// `.tex` before the metadata sidecar existed. Returns `None` if the file is
/// missing or empty. The legacy file is left in place — the user keeps a copy
/// in git history; new edits flow through `chapter.json`.
fn read_legacy_notes(tex_path: &Path) -> Option<String> {
    let mut notes_path = tex_path.to_path_buf();
    let stem = tex_path.file_stem()?.to_string_lossy().into_owned();
    notes_path.set_file_name(format!("{stem}.notes.md"));
    let s = std::fs::read_to_string(&notes_path).ok()?;
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ckwriter-book-mod-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn book_config_defaults_match_writer_legacy_layout() {
        // An older `Info/index.json` with no folder-list keys must keep
        // working: the writer's existing book has no manuscript_folders /
        // authors_folders entries on disk yet, and #0033 must not break it.
        let raw = r#"{"main_tex":"main.tex"}"#;
        let cfg: BookConfig = serde_json::from_str(raw).expect("parse legacy index.json");
        assert_eq!(cfg.manuscript_folders, vec!["Ancient", "Modern"]);
        assert_eq!(cfg.authors_folders, vec!["Info/Characters"]);
    }

    #[test]
    fn book_config_round_trips_explicit_lists() {
        let cfg = BookConfig {
            manuscript_folders: vec!["Manuscript".into()],
            authors_folders: vec!["People".into(), "Cast".into()],
            ..BookConfig::default()
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: BookConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.manuscript_folders, vec!["Manuscript"]);
        assert_eq!(back.authors_folders, vec!["People", "Cast"]);
    }

    #[test]
    fn seed_meta_computes_word_count_from_prose() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("Modern")).unwrap();
        let tex_path = root.join("Modern/010_Awakening.tex");
        // 6 prose words once `\chapter{...}` is unwrapped to its title text.
        std::fs::write(&tex_path, "\\chapter{Awakening}\n\nHe ran into the dim hall.\n").unwrap();
        let meta = load_or_seed_meta(&root, "Modern", "Awakening", &tex_path);
        // "Awakening" + "He ran into the dim hall." = 7 words.
        assert_eq!(meta.word_count, 7);
        assert!(meta.plot_notes.is_empty());
        // The seeded file is now on disk for next time.
        assert!(chapter_meta::file_path(&root, "Modern", "Awakening").exists());
    }

    #[test]
    fn seed_meta_migrates_legacy_notes_md() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("Modern")).unwrap();
        let tex_path = root.join("Modern/010_Awakening.tex");
        std::fs::write(&tex_path, "\\chapter{Awakening}\nbody\n").unwrap();
        std::fs::write(
            root.join("Modern/010_Awakening.notes.md"),
            "old scratchpad content",
        )
        .unwrap();
        let meta = load_or_seed_meta(&root, "Modern", "Awakening", &tex_path);
        assert_eq!(meta.plot_notes, "old scratchpad content");
        // Legacy file is intentionally left in place — user keeps a copy in
        // git history. We don't clean it up.
        assert!(root.join("Modern/010_Awakening.notes.md").exists());
    }

    #[test]
    fn seed_meta_skips_empty_notes_md() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("Modern")).unwrap();
        let tex_path = root.join("Modern/010_Awakening.tex");
        std::fs::write(&tex_path, "body").unwrap();
        std::fs::write(root.join("Modern/010_Awakening.notes.md"), "   \n\t\n").unwrap();
        let meta = load_or_seed_meta(&root, "Modern", "Awakening", &tex_path);
        assert!(meta.plot_notes.is_empty());
    }

    #[test]
    fn legacy_dismissals_migrate_into_per_chapter_files() {
        use crate::book::suggestions::{ChapterSuggestions, Status};

        let root = tempdir();
        std::fs::create_dir_all(root.join("Modern")).unwrap();
        // The chapter prose contains the dismissed quote; migrator should bind
        // a paragraph_id.
        let tex_path = root.join("Modern/010_Awakening.tex");
        std::fs::write(
            &tex_path,
            "\\chapter{Awakening}\n\nthe dog ran across the open field today.\n",
        )
        .unwrap();

        // Legacy file: one chapter, two pipelines, three quotes (one with no
        // matching paragraph so we exercise the paragraph_id = None path).
        let legacy = serde_json::json!({
            "by_chapter": {
                "Awakening": {
                    "voice": ["the dog ran across the open field"],
                    "prose": ["across the open field", "absent quote not in text"],
                }
            }
        });
        std::fs::create_dir_all(root.join("Info")).unwrap();
        std::fs::write(
            root.join("Info/coach-dismissals.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let chapters = vec![Chapter {
            folder: "Modern".into(),
            name: "Awakening".into(),
            include_path: "Modern/010_Awakening".into(),
            file_path: tex_path.clone(),
            display_title: "Awakening".into(),
            in_manuscript: true,
            meta: ChapterMeta::default(),
        }];
        suggestions_migrate::run(&root, &chapters).unwrap();

        // Per-chapter file exists with all three records, all Dismissed.
        let chapter = ChapterSuggestions::load(&root, "Modern", "Awakening");
        assert_eq!(chapter.records.len(), 3);
        for rec in chapter.records.values() {
            assert_eq!(rec.status, Status::Dismissed);
            assert!(rec.resolved_at.is_some());
        }
        // Two of three resolved a paragraph_id; one (the absent quote) did not.
        let with_pid = chapter
            .records
            .values()
            .filter(|r| r.paragraph_id.is_some())
            .count();
        let without_pid = chapter
            .records
            .values()
            .filter(|r| r.paragraph_id.is_none())
            .count();
        assert_eq!(with_pid, 2);
        assert_eq!(without_pid, 1);

        // Legacy file renamed; idempotent re-run is a no-op.
        assert!(!root.join("Info/coach-dismissals.json").exists());
        assert!(root.join("Info/coach-dismissals.json.migrated").exists());
        suggestions_migrate::run(&root, &chapters).unwrap();
        let again = ChapterSuggestions::load(&root, "Modern", "Awakening");
        assert_eq!(again.records.len(), 3);
    }

    #[test]
    fn existing_meta_takes_precedence_over_seed() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("Modern")).unwrap();
        let tex_path = root.join("Modern/010_Awakening.tex");
        std::fs::write(&tex_path, "fresh body").unwrap();
        // Pre-existing meta with a hand-written summary; seed should not stomp it.
        let prior = chapter_meta::ChapterMeta {
            summary: "kept".into(),
            word_count: 9999,
            ..Default::default()
        };
        chapter_meta::save(&root, "Modern", "Awakening", &prior).unwrap();
        let meta = load_or_seed_meta(&root, "Modern", "Awakening", &tex_path);
        assert_eq!(meta.summary, "kept");
        assert_eq!(meta.word_count, 9999);
    }
}
