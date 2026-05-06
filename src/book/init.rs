//! Project scaffolder (#0033).
//!
//! Creates a fresh CkWriter book in an empty directory: a minimal `main.tex`
//! with the manuscript-include sentinels, an empty `Info/manuscript.json`, an
//! `Info/index.json` carrying the new project's `BookConfig`, and a default
//! `Info/Characters/` directory. Refuses non-empty targets so a misclick can't
//! scribble into an unrelated folder; dotfiles (`.git`, `.gitignore`, IDE
//! crud) are tolerated so the writer can `git init` first.

use crate::book::manuscript::Manuscript;
use crate::book::{latex, BookConfig};
use anyhow::{anyhow, Result};
use std::path::Path;

const MAIN_TEX_TEMPLATE: &str = "\\documentclass[14pt,a4paper]{extbook}\n\
\\usepackage[top=4cm, bottom=3cm, outer=3cm, inner=3cm, headsep=30pt]{geometry}\n\
\\renewcommand{\\baselinestretch}{1.5}\n\
\\setlength{\\parskip}{2em}\n\
\n\
\\title{Untitled}\n\
\\author{}\n\
\n\
\\begin{document}\n\
\\maketitle\n\
\n\
% --- BEGIN CKWRITER MANUSCRIPT ---\n\
% --- END CKWRITER MANUSCRIPT ---\n\
\n\
\\end{document}\n";

/// Initialise a fresh CkWriter book at `root`. The directory must already
/// exist and be empty (apart from dotfiles). Returns `Ok(())` once every file
/// has been written; on failure the partial scaffold is left in place — the
/// next `init_project` call against the same directory will refuse.
pub fn init_project(root: &Path) -> Result<()> {
    if !root.exists() {
        return Err(anyhow!(
            "init_project: target does not exist: {}",
            root.display()
        ));
    }
    if !root.is_dir() {
        return Err(anyhow!(
            "init_project: target is not a directory: {}",
            root.display()
        ));
    }
    let visible_entries = std::fs::read_dir(root)?.filter_map(|e| e.ok()).any(|e| {
        e.file_name()
            .to_str()
            .map(|n| !n.starts_with('.'))
            .unwrap_or(false)
    });
    if visible_entries {
        return Err(anyhow!(
            "init_project: directory is not empty: {}",
            root.display()
        ));
    }

    // main.tex with the manuscript sentinels already in place. The empty
    // manuscript leaves nothing between them — `chapters::commit` is a no-op
    // until the writer adds a chapter.
    debug_assert!(MAIN_TEX_TEMPLATE.contains(latex::SENTINEL_BEGIN));
    debug_assert!(MAIN_TEX_TEMPLATE.contains(latex::SENTINEL_END));
    std::fs::write(root.join("main.tex"), MAIN_TEX_TEMPLATE)?;

    // Empty manuscript so seed-from-includes doesn't run on first open.
    Manuscript::default().save(root)?;

    // BookConfig overrides: a fresh project starts with no manuscript folders
    // (the writer adds them via project settings) and the default authors
    // folder. The other config keys keep their library defaults.
    let cfg = BookConfig {
        manuscript_folders: Vec::new(),
        ..BookConfig::default()
    };
    cfg.save(root)?;

    // Pre-create the canonical authors folder so the file tree has somewhere
    // to drop the first character JSON without an extra mkdir on save. Other
    // configured authors folders are created lazily on first save.
    if let Some(first) = cfg.authors_folders.first() {
        std::fs::create_dir_all(root.join(first))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "ckwriter-init-{}-{nanos}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn init_writes_minimal_scaffold() {
        let root = tempdir();
        init_project(&root).expect("scaffold");

        // main.tex with sentinels.
        let main = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert!(main.contains(latex::SENTINEL_BEGIN));
        assert!(main.contains(latex::SENTINEL_END));

        // Empty manuscript.json.
        let manuscript: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("Info/manuscript.json")).unwrap())
                .unwrap();
        assert_eq!(manuscript["chapters"], serde_json::json!([]));

        // index.json with empty manuscript folders + default authors folder.
        let cfg: BookConfig =
            serde_json::from_str(&std::fs::read_to_string(root.join("Info/index.json")).unwrap())
                .unwrap();
        assert!(cfg.manuscript_folders.is_empty());
        assert_eq!(cfg.authors_folders, vec!["Info/Characters"]);

        // Authors folder created.
        assert!(root.join("Info/Characters").is_dir());
    }

    #[test]
    fn init_refuses_non_empty_directory() {
        let root = tempdir();
        std::fs::write(root.join("foo.txt"), "hi").unwrap();
        let err = init_project(&root).expect_err("non-empty should fail");
        let msg = format!("{err}");
        assert!(msg.contains("not empty"), "{msg}");
    }

    #[test]
    fn init_tolerates_dotfiles() {
        let root = tempdir();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        init_project(&root).expect("dotfile-only directory should be acceptable");
        assert!(root.join("main.tex").exists());
    }

    #[test]
    fn init_refuses_missing_directory() {
        let parent = tempdir();
        let missing = parent.join("does-not-exist");
        let err = init_project(&missing).expect_err("missing dir should fail");
        assert!(format!("{err}").contains("does not exist"));
    }
}
