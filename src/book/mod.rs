pub mod entity;
pub mod latex;

use anyhow::{anyhow, Result};
use entity::{Entities, Entity, EntityKind};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Chapter {
    #[allow(dead_code)]
    pub include_path: String,
    pub file_path: PathBuf,
    pub display_title: String,
    pub group: String,
    pub in_manuscript: bool,
}

pub struct Book {
    pub root: PathBuf,
    #[allow(dead_code)]
    pub main_tex: PathBuf,
    pub chapters: Vec<Chapter>,
    pub entities: Entities,
    pub voice_prompt: String,
    pub roadmap: String,
    pub config: BookConfig,
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
            return Err(anyhow!(
                "main TeX file not found: {}",
                main_tex.display()
            ));
        }

        let main_text = std::fs::read_to_string(&main_tex)?;
        let included = latex::parse_includes(&main_text);

        let mut chapters: Vec<Chapter> = Vec::new();
        for inc in &included {
            let file = root.join(format!("{inc}.tex"));
            let title = read_chapter_title(&file).unwrap_or_else(|| inc.clone());
            let group = group_of(inc);
            chapters.push(Chapter {
                include_path: inc.clone(),
                file_path: file,
                display_title: title,
                group,
                in_manuscript: true,
            });
        }

        // Loose .tex files: anything in Ancient/ or Modern/ not already included.
        for sub in &["Ancient", "Modern"] {
            let dir = root.join(sub);
            if !dir.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&dir)?.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("tex") {
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let inc = format!("{sub}/{stem}");
                    if !included.iter().any(|i| i == &inc) {
                        let title = read_chapter_title(&p).unwrap_or_else(|| stem.into());
                        chapters.push(Chapter {
                            include_path: inc.clone(),
                            file_path: p,
                            display_title: title,
                            group: sub.to_string(),
                            in_manuscript: false,
                        });
                    }
                }
            }
        }

        let entities = Entities::load(root);

        let voice_prompt =
            std::fs::read_to_string(root.join(&config.voice_prompt_file)).unwrap_or_default();
        let roadmap = std::fs::read_to_string(root.join(&config.roadmap_file)).unwrap_or_default();

        Ok(Self {
            root: root.to_path_buf(),
            main_tex,
            chapters,
            entities,
            voice_prompt,
            roadmap,
            config,
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

    pub fn entity(&self, id: &str) -> Option<&Entity> {
        self.entities.get(id)
    }

    pub fn save_entity(&mut self, e: Entity) -> Result<()> {
        self.entities.save(&self.root, e)
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

fn group_of(include_path: &str) -> String {
    include_path
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}
