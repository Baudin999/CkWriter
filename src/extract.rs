use crate::book::entity::{Entities, Entity, EntityKind};
use crate::book::latex;
use aho_corasick::{AhoCorasick, MatchKind};

#[derive(Debug, Clone)]
pub struct EntityHit {
    pub start: usize,
    pub end: usize,
    pub entity_id: String,
    pub kind: EntityKind,
}

pub struct EntityMatcher {
    ac: Option<AhoCorasick>,
    /// pattern_index → (entity_id, kind)
    targets: Vec<(String, EntityKind)>,
}

impl EntityMatcher {
    pub fn build(entities: &Entities) -> Self {
        let mut patterns: Vec<String> = Vec::new();
        let mut targets: Vec<(String, EntityKind)> = Vec::new();
        for e in entities.by_id.values() {
            if !matches!(e.kind, EntityKind::Character | EntityKind::Location) {
                continue;
            }
            for term in e.match_terms() {
                if term.len() < 2 {
                    continue;
                }
                patterns.push(term);
                targets.push((e.id.clone(), e.kind));
            }
        }

        let ac = if patterns.is_empty() {
            None
        } else {
            AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .ascii_case_insensitive(false)
                .build(&patterns)
                .ok()
        };
        Self { ac, targets }
    }

    pub fn find(&self, text: &str) -> Vec<EntityHit> {
        let Some(ac) = &self.ac else {
            return Vec::new();
        };
        let skips = latex::skip_ranges(text);
        let mut hits: Vec<EntityHit> = Vec::new();
        for m in ac.find_iter(text) {
            let (start, end) = (m.start(), m.end());

            // Reject overlap with LaTeX-skip regions.
            if skips.iter().any(|(a, b)| start < *b && end > *a) {
                continue;
            }
            // Word-boundary check: byte before start must not be alnum, byte after end likewise.
            if !is_word_boundary(text, start, end) {
                continue;
            }
            let (id, kind) = &self.targets[m.pattern().as_usize()];
            hits.push(EntityHit {
                start,
                end,
                entity_id: id.clone(),
                kind: *kind,
            });
        }
        hits
    }
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let before_ok = if start == 0 {
        true
    } else {
        let b = bytes[start - 1];
        !(b.is_ascii_alphanumeric() || b == b'_')
    };
    let after_ok = if end >= bytes.len() {
        true
    } else {
        let b = bytes[end];
        !(b.is_ascii_alphanumeric() || b == b'_')
    };
    before_ok && after_ok
}

/// Find the entity hit at character byte offset `byte` (cursor position).
pub fn hit_at(hits: &[EntityHit], byte: usize) -> Option<&EntityHit> {
    hits.iter().find(|h| byte >= h.start && byte < h.end)
}

/// Lightweight candidate proper-noun finder (capitalized word not at sentence start).
#[allow(dead_code)]
pub fn candidates(text: &str, known_aliases: &[String]) -> Vec<(usize, usize, String)> {
    let known: std::collections::HashSet<&str> = known_aliases.iter().map(|s| s.as_str()).collect();
    let stop: std::collections::HashSet<&str> = STOPLIST.iter().copied().collect();
    let skips = latex::skip_ranges(text);

    let re = regex::Regex::new(r"\b[A-Z][a-z]{2,}(?:\s[A-Z][a-z]{2,})?\b").unwrap();
    let mut out = Vec::new();
    let mut last_pos: Option<usize> = None;
    for m in re.find_iter(text) {
        let (s, e) = (m.start(), m.end());
        if skips.iter().any(|(a, b)| s < *b && e > *a) {
            continue;
        }
        let word = &text[s..e];
        if known.contains(word) || stop.contains(word) {
            last_pos = Some(e);
            continue;
        }
        // Skip if preceded by '.', '?', '!', or start-of-text (likely sentence start).
        let trimmed_before = text[..s].trim_end();
        let prev_ch = trimmed_before.chars().last();
        let is_sentence_start = matches!(prev_ch, None | Some('.') | Some('!') | Some('?'));
        if is_sentence_start {
            last_pos = Some(e);
            continue;
        }
        out.push((s, e, word.to_string()));
        last_pos = Some(e);
    }
    let _ = last_pos;
    out
}

#[allow(dead_code)]
const STOPLIST: &[&str] = &[
    "The", "And", "But", "She", "He", "His", "Her", "Their", "They", "It", "I", "We", "You",
    "There", "Then", "When", "Where", "While", "After", "Before", "With", "Without", "Into",
    "Onto", "Upon", "From", "About", "Above", "Below", "Under", "Over", "Across", "Around",
    "Behind", "Beside", "Between", "Through", "Toward", "Towards",
];

/// Aggregated occurrence count per entity for the "in scope" panel.
pub fn frequency_map(hits: &[EntityHit]) -> Vec<(String, EntityKind, usize)> {
    use std::collections::HashMap;
    let mut counts: HashMap<(String, EntityKind), usize> = HashMap::new();
    for h in hits {
        *counts.entry((h.entity_id.clone(), h.kind)).or_insert(0) += 1;
    }
    let mut v: Vec<_> = counts.into_iter().map(|((id, k), c)| (id, k, c)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    v
}

pub fn by_kind(freqs: &[(String, EntityKind, usize)], kind: EntityKind) -> Vec<(&str, usize)> {
    freqs
        .iter()
        .filter(|(_, k, _)| *k == kind)
        .map(|(id, _, c)| (id.as_str(), *c))
        .collect()
}

#[allow(dead_code)]
fn _entity_lookup(_e: &Entity) {} // silence unused import in tests
