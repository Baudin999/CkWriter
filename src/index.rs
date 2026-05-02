//! Cross-chapter occurrence index for entities.
//!
//! Built once after `Book::open` and rebuilt whenever the entity DB or a
//! chapter file changes. For each entity (by id) we keep a list of every
//! match across every chapter file, with line number and a small snippet so
//! the inspector can render a clickable list.

use crate::book::{latex, Book};
use crate::extract::EntityMatcher;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ChapterOccurrence {
    pub chapter_path: PathBuf,
    pub chapter_title: String,
    /// 1-based line number into the chapter file, suitable for `jump_to_source`.
    pub line: u32,
    pub snippet: String,
}

#[derive(Debug, Default)]
pub struct CrossChapterIndex {
    by_entity: HashMap<String, Vec<ChapterOccurrence>>,
}

impl CrossChapterIndex {
    pub fn build(book: &Book, matcher: &EntityMatcher) -> Self {
        let mut by_entity: HashMap<String, Vec<ChapterOccurrence>> = HashMap::new();
        for ch in &book.chapters {
            let text = match std::fs::read_to_string(&ch.file_path) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!(
                        "index: cannot read {}: {e}",
                        ch.file_path.display()
                    );
                    continue;
                }
            };
            // Skip-ranges are honored by the matcher itself.
            for h in matcher.find(&text) {
                let line = line_at(&text, h.start);
                let snippet = snippet_around(&text, h.start, h.end, 60);
                by_entity
                    .entry(h.entity_id.clone())
                    .or_default()
                    .push(ChapterOccurrence {
                        chapter_path: ch.file_path.clone(),
                        chapter_title: ch.display_title.clone(),
                        line,
                        snippet,
                    });
            }
        }
        // Sort each entity's occurrences by chapter title then line so the UI
        // is stable across rebuilds.
        for v in by_entity.values_mut() {
            v.sort_by(|a, b| {
                a.chapter_title
                    .cmp(&b.chapter_title)
                    .then(a.line.cmp(&b.line))
            });
        }
        Self { by_entity }
    }

    pub fn for_entity(&self, id: &str) -> &[ChapterOccurrence] {
        self.by_entity.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn total_occurrences(&self, id: &str) -> usize {
        self.by_entity.get(id).map(|v| v.len()).unwrap_or(0)
    }

    pub fn distinct_chapter_count(&self, id: &str) -> usize {
        let Some(v) = self.by_entity.get(id) else { return 0 };
        let mut chapters: Vec<&PathBuf> = v.iter().map(|o| &o.chapter_path).collect();
        chapters.sort();
        chapters.dedup();
        chapters.len()
    }

    pub fn entity_count(&self) -> usize {
        self.by_entity.len()
    }

    pub fn total_occurrences_all(&self) -> usize {
        self.by_entity.values().map(|v| v.len()).sum()
    }
}

/// 1-based line number of `byte_offset` in `text`.
fn line_at(text: &str, byte_offset: usize) -> u32 {
    let cap = byte_offset.min(text.len());
    let mut line = 1u32;
    for &b in &text.as_bytes()[..cap] {
        if b == b'\n' {
            line += 1;
        }
    }
    line
}

/// A short, single-line snippet around `[start, end)` for display.
/// LaTeX commands inside the window are stripped via `latex::to_prose` so the
/// snippet reads cleanly.
fn snippet_around(text: &str, start: usize, end: usize, pad: usize) -> String {
    let s = floor_char_boundary(text, start.saturating_sub(pad));
    let e = ceil_char_boundary(text, end.saturating_add(pad).min(text.len()));
    let raw = &text[s..e];
    let cleaned = latex::to_prose(raw);
    let one_line: String = cleaned
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = one_line;
    if s > 0 {
        out.insert(0, '…');
    }
    if e < text.len() {
        out.push('…');
    }
    out
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_at_counts_newlines() {
        let s = "alpha\nbeta\ngamma";
        assert_eq!(line_at(s, 0), 1);
        assert_eq!(line_at(s, 6), 2); // first byte of "beta"
        assert_eq!(line_at(s, 11), 3); // first byte of "gamma"
        assert_eq!(line_at(s, 999), 3); // past-end clamps
    }

    #[test]
    fn snippet_strips_latex_and_collapses_lines() {
        let text = "He said \\emph{Wua} walked east.\nNew paragraph.";
        let start = text.find("Wua").unwrap();
        let end = start + "Wua".len();
        let snip = snippet_around(text, start, end, 80);
        // The \emph wrapper is gone, the result is one line.
        assert!(snip.contains("Wua"));
        assert!(!snip.contains("\\emph"));
        assert!(!snip.contains('\n'));
    }
}
