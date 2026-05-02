use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
}

impl FileNode {
    pub fn build(root: &Path) -> Self {
        let name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        Self {
            name,
            path: root.to_path_buf(),
            is_dir: true,
            children: read_dir_filtered(root),
        }
    }
}

fn read_dir_filtered(dir: &Path) -> Vec<FileNode> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out: Vec<FileNode> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_visible(&name, is_dir) {
            continue;
        }
        let children = if is_dir { read_dir_filtered(&path) } else { Vec::new() };
        out.push(FileNode { name, path, is_dir, children });
    }

    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    out
}

fn is_visible(name: &str, is_dir: bool) -> bool {
    if name.starts_with('.') {
        return false;
    }
    if is_dir {
        // Generated/build/scratch directories that aren't book content.
        return !matches!(name, "output" | "target" | "node_modules");
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("tex" | "md" | "txt" | "json" | "toml" | "yml" | "yaml" | "bib"))
}
