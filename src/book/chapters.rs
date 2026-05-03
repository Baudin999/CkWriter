//! Chapter file operations: add, delete, exclude/include, reorder, and the
//! normalize pass that keeps disk filenames consistent with manuscript order.
//!
//! ## Filename invariants
//!
//! - **Manuscript chapter**: `<folder>/NNN_<Name>.tex` where `NNN` is
//!   `(folder_pos + 1) * 10` zero-padded to 3 digits. `folder_pos` is the
//!   chapter's index among entries of its folder in `manuscript.chapters`.
//! - **Orphan chapter**: `<folder>/<Name>.tex` (no numeric prefix). An orphan
//!   is a `.tex` file in a managed folder that doesn't appear in the
//!   manuscript. Keeping orphans un-numbered avoids collisions when a
//!   manuscript chapter is renumbered into a slot the orphan used to occupy.
//!
//! ## Why a normalize pass
//!
//! Every mutating operation (add/delete/exclude/reorder) updates
//! `manuscript.json` first, then runs `normalize` to make the disk match.
//! Normalize is a single function that does the work for every operation —
//! the operations themselves only have to mutate the manuscript and
//! optionally create/delete a file. This keeps the filename invariant in
//! one place instead of scattered across each op.

use crate::book::latex;
use crate::book::manuscript::{
    self, camel_case, derive_filename, derive_include_path, strip_number_prefix, ChapterRef,
    Manuscript,
};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Public per-folder summary used by the file-tree UI: every chapter that
/// lives in this folder, ordered as the file system would list it after a
/// successful normalize (manuscript first by position, orphans last
/// alphabetically).
#[derive(Debug, Clone, Default)]
pub struct FolderListing {
    pub manuscript: Vec<FolderEntry>,
    pub orphans: Vec<FolderEntry>,
}

#[derive(Debug, Clone)]
pub struct FolderEntry {
    pub name: String,
    pub file_path: PathBuf,
}

/// Locate the on-disk file for a `(folder, name)` pair, regardless of whether
/// it currently has a numeric prefix. Used during normalize to find the
/// pre-rename file. Returns the first matching `.tex` file or `None`.
pub fn find_chapter_file(root: &Path, folder: &str, name: &str) -> Option<PathBuf> {
    let dir = root.join(folder);
    let rd = std::fs::read_dir(&dir).ok()?;
    let target_unnumbered = format!("{name}.tex");
    let target_suffix = format!("_{name}.tex");
    for entry in rd.flatten() {
        let p = entry.path();
        let fname = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if fname == target_unnumbered || fname.ends_with(&target_suffix) {
            return Some(p);
        }
    }
    None
}

/// Discover every `.tex` file in a managed folder and bucket each one by name.
/// The map's key is the name without numeric prefix (so `010_Wua.tex` and a
/// future `020_Wua.tex` would collide and produce a warning). Used by
/// `normalize` to plan renames and by `Book::open` to enumerate orphans.
pub fn scan_folder(root: &Path, folder: &str) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let dir = root.join(folder);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("tex") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let key = strip_number_prefix(stem).to_string();
        if out.insert(key.clone(), p.clone()).is_some() {
            log::warn!("duplicate chapter name {key:?} under {folder:?}; ignoring earlier file");
        }
    }
    out
}

/// Reconcile disk to manuscript: rename every chapter file to its derived
/// path, drop number prefixes from orphans, and verify each manuscript entry
/// has a backing file. Idempotent — calling it on an already-normalized tree
/// performs zero file system writes. Two-phase rename via a `__staging_*`
/// intermediate avoids collisions when files swap numbers (Wua at 010 wants
/// 020 while Arrival at 020 wants 010).
pub fn normalize(root: &Path, manuscript: &Manuscript) -> Result<()> {
    // Plan: for every name in every managed folder, what file path it should
    // end up at. Manuscript entries are numbered by their folder-position;
    // orphans drop the number entirely.
    let mut planned: Vec<(PathBuf, PathBuf)> = Vec::new(); // (current, target)
    let mut missing: Vec<ChapterRef> = Vec::new();

    for folder in manuscript::MANAGED_FOLDERS {
        let folder_str = (*folder).to_string();
        let scan = scan_folder(root, folder);

        // Manuscript chapters in this folder, in manuscript order.
        let mut folder_pos = 0usize;
        for c in &manuscript.chapters {
            if c.folder != folder_str {
                continue;
            }
            let target = root.join(folder).join(derive_filename(folder_pos, &c.name));
            match scan.get(&c.name) {
                Some(current) => {
                    if current != &target {
                        planned.push((current.clone(), target));
                    }
                }
                None => missing.push(c.clone()),
            }
            folder_pos += 1;
        }

        // Orphans: anything in scan that's not in manuscript.
        let mut orphan_names: Vec<&String> = scan
            .keys()
            .filter(|k| !manuscript.contains(folder, k))
            .collect();
        orphan_names.sort();
        for name in orphan_names {
            let target = root.join(folder).join(format!("{name}.tex"));
            let current = &scan[name];
            if current != &target {
                planned.push((current.clone(), target));
            }
        }
    }

    if !missing.is_empty() {
        for m in &missing {
            log::warn!(
                "manuscript references missing file: {}/{}",
                m.folder,
                m.name
            );
        }
    }

    if planned.is_empty() {
        return Ok(());
    }

    // Two-phase: rename every (current → target) pair through a unique
    // staging name first so cycles (A↔B swap) can't collide on phase 1.
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(planned.len());
    for (i, (current, target)) in planned.iter().enumerate() {
        let parent = current
            .parent()
            .ok_or_else(|| anyhow!("rename source has no parent: {}", current.display()))?;
        let staging = parent.join(format!("__ckwriter_staging_{i}.tex"));
        std::fs::rename(current, &staging).map_err(|e| {
            anyhow!(
                "stage rename {} -> {}: {e}",
                current.display(),
                staging.display()
            )
        })?;
        staged.push((staging, target.clone()));
    }
    for (staging, target) in &staged {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(staging, target).map_err(|e| {
            anyhow!(
                "final rename {} -> {}: {e}",
                staging.display(),
                target.display()
            )
        })?;
    }

    Ok(())
}

/// Build the ordered list of include paths corresponding to the current
/// manuscript. This is what gets written between the `main.tex` sentinels.
pub fn manuscript_include_paths(manuscript: &Manuscript) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(manuscript.chapters.len());
    let mut per_folder_pos: HashMap<&str, usize> = HashMap::new();
    for c in &manuscript.chapters {
        let pos = per_folder_pos.entry(c.folder.as_str()).or_insert(0);
        out.push(derive_include_path(*pos, &c.folder, &c.name));
        *pos += 1;
    }
    out
}

/// Read `main.tex`, sync the include block from `manuscript`, write back.
/// No-op if the file content is already up to date.
pub fn write_main_tex(root: &Path, main_tex_name: &str, manuscript: &Manuscript) -> Result<()> {
    let path = root.join(main_tex_name);
    let current =
        std::fs::read_to_string(&path).map_err(|e| anyhow!("read {}: {e}", path.display()))?;
    let synced = latex::sync_main_tex(&current, &manuscript_include_paths(manuscript));
    if synced != current {
        std::fs::write(&path, synced).map_err(|e| anyhow!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// One-shot: update manuscript.json + main.tex + filenames after `manuscript`
/// has been mutated by the caller. Every public op below routes through
/// here so the three pieces stay in lockstep.
pub fn commit(root: &Path, main_tex_name: &str, manuscript: &Manuscript) -> Result<()> {
    normalize(root, manuscript)?;
    manuscript.save(root)?;
    write_main_tex(root, main_tex_name, manuscript)?;
    Ok(())
}

/// Generate a unique CamelCase name for a new chapter under `folder`. If
/// `Wua` already exists, returns `Wua2`, `Wua3`, etc.
fn unique_name(root: &Path, folder: &str, base: &str) -> String {
    let scan = scan_folder(root, folder);
    if !scan.contains_key(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}{n}");
        if !scan.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

const NEW_CHAPTER_TEMPLATE: &str = "\\chapter{TITLE}\n\n";

/// Add a new chapter to the manuscript. Creates `Folder/NNN_Name.tex` with a
/// minimal `\chapter{title}` skeleton, where `NNN` reflects the new chapter's
/// folder-position (it's appended at the end of the manuscript). Returns the
/// new ChapterRef for the caller to focus on.
pub fn add_chapter(
    root: &Path,
    main_tex_name: &str,
    manuscript: &mut Manuscript,
    folder: &str,
    title: &str,
) -> Result<ChapterRef> {
    if !manuscript::MANAGED_FOLDERS.contains(&folder) {
        return Err(anyhow!("unmanaged folder: {folder}"));
    }
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("chapter title is empty"));
    }
    let base = camel_case(trimmed);
    if base.is_empty() {
        return Err(anyhow!(
            "chapter title produced empty camelcase name: {trimmed:?}"
        ));
    }
    let name = unique_name(root, folder, &base);

    let folder_pos = manuscript
        .chapters
        .iter()
        .filter(|c| c.folder == folder)
        .count();
    let target = root.join(folder).join(derive_filename(folder_pos, &name));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = NEW_CHAPTER_TEMPLATE.replace("TITLE", trimmed);
    std::fs::write(&target, body).map_err(|e| anyhow!("write {}: {e}", target.display()))?;

    let entry = ChapterRef {
        folder: folder.to_string(),
        name,
    };
    manuscript.chapters.push(entry.clone());
    commit(root, main_tex_name, manuscript)?;
    Ok(entry)
}

/// Remove a chapter from the manuscript and delete its file. Other chapters
/// in the same folder shift up to fill the gap.
pub fn delete_chapter(
    root: &Path,
    main_tex_name: &str,
    manuscript: &mut Manuscript,
    folder: &str,
    name: &str,
) -> Result<()> {
    if let Some(file) = find_chapter_file(root, folder, name) {
        std::fs::remove_file(&file).map_err(|e| anyhow!("delete {}: {e}", file.display()))?;
    } else {
        log::warn!("delete_chapter: no file for {folder}/{name}");
    }
    manuscript
        .chapters
        .retain(|c| !(c.folder == folder && c.name == name));
    commit(root, main_tex_name, manuscript)?;
    Ok(())
}

/// Drop a chapter from the manuscript without touching the file on disk.
/// On commit, normalize will rename the orphan to drop its number prefix.
pub fn exclude_chapter(
    root: &Path,
    main_tex_name: &str,
    manuscript: &mut Manuscript,
    folder: &str,
    name: &str,
) -> Result<()> {
    let was_present = manuscript
        .chapters
        .iter()
        .any(|c| c.folder == folder && c.name == name);
    if !was_present {
        return Err(anyhow!("not in manuscript: {folder}/{name}"));
    }
    manuscript
        .chapters
        .retain(|c| !(c.folder == folder && c.name == name));
    commit(root, main_tex_name, manuscript)?;
    Ok(())
}

/// Append an orphan chapter back into the manuscript at the end. On commit,
/// normalize gives the file its number prefix.
pub fn include_chapter(
    root: &Path,
    main_tex_name: &str,
    manuscript: &mut Manuscript,
    folder: &str,
    name: &str,
) -> Result<()> {
    if manuscript.contains(folder, name) {
        return Err(anyhow!("already in manuscript: {folder}/{name}"));
    }
    if find_chapter_file(root, folder, name).is_none() {
        return Err(anyhow!("no file on disk for {folder}/{name}"));
    }
    manuscript.chapters.push(ChapterRef {
        folder: folder.to_string(),
        name: name.to_string(),
    });
    commit(root, main_tex_name, manuscript)?;
    Ok(())
}

/// Replace the manuscript order. Each entry must already be a member of the
/// current manuscript — this op reorders, it doesn't include or exclude.
/// On commit, normalize renumbers files to reflect the new order.
pub fn reorder_manuscript(
    root: &Path,
    main_tex_name: &str,
    manuscript: &mut Manuscript,
    new_order: Vec<ChapterRef>,
) -> Result<()> {
    if new_order.len() != manuscript.chapters.len() {
        return Err(anyhow!(
            "reorder length mismatch: got {}, expected {}",
            new_order.len(),
            manuscript.chapters.len()
        ));
    }
    for c in &new_order {
        if !manuscript.contains(&c.folder, &c.name) {
            return Err(anyhow!(
                "reorder contains chapter not in manuscript: {}/{}",
                c.folder,
                c.name
            ));
        }
    }
    manuscript.chapters = new_order;
    commit(root, main_tex_name, manuscript)?;
    Ok(())
}

/// Inventory used by the file-tree UI: every chapter known under each
/// managed folder, split into manuscript-vs-orphan and sorted appropriately.
pub fn list_folders(root: &Path, manuscript: &Manuscript) -> HashMap<String, FolderListing> {
    let mut out: HashMap<String, FolderListing> = HashMap::new();
    for folder in manuscript::MANAGED_FOLDERS {
        let scan = scan_folder(root, folder);
        let mut listing = FolderListing::default();

        for c in manuscript.chapters.iter().filter(|c| c.folder == *folder) {
            if let Some(path) = scan.get(&c.name) {
                listing.manuscript.push(FolderEntry {
                    name: c.name.clone(),
                    file_path: path.clone(),
                });
            }
        }

        let mut orphan_names: Vec<&String> = scan
            .keys()
            .filter(|k| !manuscript.contains(folder, k))
            .collect();
        orphan_names.sort();
        for name in orphan_names {
            listing.orphans.push(FolderEntry {
                name: name.clone(),
                file_path: scan[name].clone(),
            });
        }

        out.insert((*folder).to_string(), listing);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        // Per-process atomic counter so parallel cargo test runs can't collide
        // on a same-nanosecond seed.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "ckwriter-chapters-{}-{nanos}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        for f in manuscript::MANAGED_FOLDERS {
            std::fs::create_dir_all(p.join(f)).unwrap();
        }
        std::fs::write(
            p.join("main.tex"),
            "\\begin{document}\n\\maketitle\n\\end{document}\n",
        )
        .unwrap();
        p
    }

    #[test]
    fn add_creates_file_with_chapter_skeleton() {
        let root = tempdir();
        let mut m = Manuscript::default();
        let entry =
            add_chapter(&root, "main.tex", &mut m, "Ancient", "First Encounter").expect("add");
        assert_eq!(entry.name, "FirstEncounter");
        let path = root.join("Ancient/010_FirstEncounter.tex");
        assert!(path.exists(), "{} should exist", path.display());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\\chapter{First Encounter}"));
        assert_eq!(m.chapters.len(), 1);

        let main_tex = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert!(main_tex.contains("\\include{Ancient/010_FirstEncounter}"));
    }

    #[test]
    fn add_with_collision_picks_unique_name() {
        let root = tempdir();
        let mut m = Manuscript::default();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        let second = add_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").expect("second add");
        assert_eq!(second.name, "Wua2");
        assert!(root.join("Ancient/010_Wua.tex").exists());
        assert!(root.join("Ancient/020_Wua2.tex").exists());
    }

    #[test]
    fn delete_removes_file_and_compacts_numbers() {
        let root = tempdir();
        let mut m = Manuscript::default();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Arrival").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "FirstEncounter").unwrap();

        delete_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        assert!(!root.join("Ancient/020_Wua.tex").exists());
        // FirstEncounter slides up from 030 → 020.
        assert!(root.join("Ancient/010_Arrival.tex").exists());
        assert!(root.join("Ancient/020_FirstEncounter.tex").exists());
        assert!(!root.join("Ancient/030_FirstEncounter.tex").exists());
    }

    #[test]
    fn exclude_drops_number_prefix_then_include_restores_it() {
        let root = tempdir();
        let mut m = Manuscript::default();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Arrival").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        exclude_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();

        // Wua keeps its file but loses its number.
        assert!(!root.join("Ancient/020_Wua.tex").exists());
        assert!(root.join("Ancient/Wua.tex").exists());
        // Arrival stays at 010.
        assert!(root.join("Ancient/010_Arrival.tex").exists());
        // main.tex doesn't include Wua anymore.
        let main_tex = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert!(!main_tex.contains("Wua"));

        include_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        assert!(!root.join("Ancient/Wua.tex").exists());
        assert!(root.join("Ancient/020_Wua.tex").exists());
    }

    #[test]
    fn reorder_renumbers_files_per_folder() {
        let root = tempdir();
        let mut m = Manuscript::default();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Arrival").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Modern", "NewYorkCity").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Wua").unwrap();
        // Now: Ancient = [Arrival(010), Wua(020)], Modern = [NewYorkCity(010)].

        // Swap the two Ancients.
        let new_order = vec![
            ChapterRef {
                folder: "Ancient".into(),
                name: "Wua".into(),
            },
            ChapterRef {
                folder: "Modern".into(),
                name: "NewYorkCity".into(),
            },
            ChapterRef {
                folder: "Ancient".into(),
                name: "Arrival".into(),
            },
        ];
        reorder_manuscript(&root, "main.tex", &mut m, new_order).unwrap();

        // Ancient slice is now Wua(010), Arrival(020).
        assert!(root.join("Ancient/010_Wua.tex").exists());
        assert!(root.join("Ancient/020_Arrival.tex").exists());
        assert!(!root.join("Ancient/010_Arrival.tex").exists());
        // Modern slice unchanged.
        assert!(root.join("Modern/010_NewYorkCity.tex").exists());

        let main_tex = std::fs::read_to_string(root.join("main.tex")).unwrap();
        let begin = main_tex
            .find(latex::SENTINEL_BEGIN)
            .expect("begin sentinel");
        let end = main_tex.find(latex::SENTINEL_END).expect("end sentinel");
        let block = &main_tex[begin..end];
        // The interleaved order is reflected in main.tex.
        let i_wua = block.find("Ancient/010_Wua").unwrap();
        let i_nyc = block.find("Modern/010_NewYorkCity").unwrap();
        let i_arr = block.find("Ancient/020_Arrival").unwrap();
        assert!(i_wua < i_nyc && i_nyc < i_arr);
    }

    #[test]
    fn normalize_is_idempotent() {
        let root = tempdir();
        let mut m = Manuscript::default();
        add_chapter(&root, "main.tex", &mut m, "Ancient", "Arrival").unwrap();
        add_chapter(&root, "main.tex", &mut m, "Modern", "NewYorkCity").unwrap();

        // Snapshot dir-listing.
        let snap = |dir: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        let before_a = snap(&root.join("Ancient"));
        let before_m = snap(&root.join("Modern"));

        normalize(&root, &m).unwrap();
        normalize(&root, &m).unwrap();

        assert_eq!(snap(&root.join("Ancient")), before_a);
        assert_eq!(snap(&root.join("Modern")), before_m);
    }

    #[test]
    fn normalize_handles_swap_via_staging() {
        let root = tempdir();
        // Pre-place files numbered backwards relative to manuscript order.
        std::fs::write(root.join("Ancient/010_Wua.tex"), "\\chapter{Wua}\nbody").unwrap();
        std::fs::write(
            root.join("Ancient/020_Arrival.tex"),
            "\\chapter{Arrival}\nbody",
        )
        .unwrap();

        // Manuscript wants Arrival(010), Wua(020) — a full swap.
        let m = Manuscript {
            chapters: vec![
                ChapterRef {
                    folder: "Ancient".into(),
                    name: "Arrival".into(),
                },
                ChapterRef {
                    folder: "Ancient".into(),
                    name: "Wua".into(),
                },
            ],
        };
        normalize(&root, &m).unwrap();

        assert!(root.join("Ancient/010_Arrival.tex").exists());
        assert!(root.join("Ancient/020_Wua.tex").exists());
        // No staging crud left behind.
        let leftovers: Vec<_> = std::fs::read_dir(root.join("Ancient"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("__ckwriter_staging_")
            })
            .collect();
        assert!(leftovers.is_empty());
    }

    /// First-open migration on a tree that already has assorted gap-numbered
    /// chapters and a parking comment in main.tex — the shape of the writer's
    /// existing project. Verifies normalize doesn't lose files, the include
    /// block gets sentinel-wrapped, and the comment-parked include survives
    /// outside the block.
    #[test]
    fn first_open_migration_preserves_orphans_and_comments() {
        let root = tempdir();
        // Pre-existing files with the writer's gap-based numbering.
        for (folder, file, title) in [
            ("Ancient", "000_Arrival.tex", "Arrival"),
            ("Ancient", "001_Wua.tex", "Wua"),
            ("Ancient", "100_BeforeTheAttack.tex", "BeforeTheAttack"),
            ("Modern", "010_NewYorkCity.tex", "NewYorkCity"),
            ("Modern", "030_LionsDen.tex", "LionsDen"),
        ] {
            std::fs::write(
                root.join(folder).join(file),
                format!("\\chapter{{{title}}}\n\nbody\n"),
            )
            .unwrap();
        }
        // An orphan: present on disk, not in main.tex.
        std::fs::write(
            root.join("Ancient/050_DraftIdeas.tex"),
            "\\chapter{DraftIdeas}\nbody\n",
        )
        .unwrap();

        let main_tex_src = "\\documentclass{book}\n\\begin{document}\n\\maketitle\n\
            % \\include{Context}\n\
            \\include{Ancient/000_Arrival}\n\
            \\include{Ancient/001_Wua}\n\
            \\include{Ancient/100_BeforeTheAttack}\n\
            \n\
            \\include{Modern/010_NewYorkCity}\n\
            \\include{Modern/030_LionsDen}\n\
            \\end{document}\n";
        std::fs::write(root.join("main.tex"), main_tex_src).unwrap();

        // Simulate the migration step Book::open does: seed manuscript from
        // main.tex includes, then commit (normalize + save + main.tex sync).
        let mut m = Manuscript::default();
        for inc in latex::parse_includes(main_tex_src) {
            let Some((folder, stem)) = inc.split_once('/') else {
                continue;
            };
            if !manuscript::MANAGED_FOLDERS.contains(&folder) {
                continue;
            }
            m.chapters.push(ChapterRef {
                folder: folder.to_string(),
                name: manuscript::strip_number_prefix(stem).to_string(),
            });
        }
        commit(&root, "main.tex", &m).unwrap();

        // Manuscript chapters get stride-10 numbers based on folder position.
        assert!(root.join("Ancient/010_Arrival.tex").exists());
        assert!(root.join("Ancient/020_Wua.tex").exists());
        assert!(root.join("Ancient/030_BeforeTheAttack.tex").exists());
        assert!(root.join("Modern/010_NewYorkCity.tex").exists());
        assert!(root.join("Modern/020_LionsDen.tex").exists());
        // Old gap-numbered names are gone.
        assert!(!root.join("Ancient/000_Arrival.tex").exists());
        assert!(!root.join("Ancient/001_Wua.tex").exists());
        assert!(!root.join("Ancient/100_BeforeTheAttack.tex").exists());
        assert!(!root.join("Modern/030_LionsDen.tex").exists());
        // Orphan keeps its file but loses its number.
        assert!(!root.join("Ancient/050_DraftIdeas.tex").exists());
        assert!(root.join("Ancient/DraftIdeas.tex").exists());

        let synced = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert!(synced.contains(latex::SENTINEL_BEGIN));
        assert!(synced.contains(latex::SENTINEL_END));
        // Commented-out parking line survives outside the wrapped block.
        assert!(synced.contains("% \\include{Context}"));
        // The include block reflects manuscript order, not the original.
        assert!(synced.contains("\\include{Ancient/010_Arrival}"));
        assert!(synced.contains("\\include{Modern/020_LionsDen}"));
        // Orphan is NOT in main.tex.
        assert!(!synced.contains("DraftIdeas"));

        // Idempotent: running commit again produces no changes on disk.
        let before = std::fs::read_to_string(root.join("main.tex")).unwrap();
        commit(&root, "main.tex", &m).unwrap();
        let after = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert_eq!(before, after);
    }
}
