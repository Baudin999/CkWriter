//! Ollama-driven character extraction.
//!
//! Two halves:
//!  1. A strict-JSON prompt that asks the model for every named character it
//!     can find in a chunk of prose, plus aliases and a one-line role.
//!  2. A diff against the on-disk `Entities` database so the UI can show the
//!     user only the characters that aren't already known.

use crate::book::entity::{slugify, Entities, Entity, EntityKind};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawCharacter {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub voice_notes: String,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawCharacters {
    #[serde(default)]
    pub characters: Vec<RawCharacter>,
}

pub fn parse_characters(buf: &str) -> Option<RawCharacters> {
    super::parse::parse_json_object(buf, "characters")
}

/// One LLM-proposed character, paired with its lookup verdict against the DB.
#[derive(Debug, Clone)]
pub struct ProposedCharacter {
    pub raw: RawCharacter,
    pub verdict: ProposalVerdict,
    pub status: ProposalStatus,
}

#[derive(Debug, Clone)]
pub enum ProposalVerdict {
    /// No existing entity matched any of the proposed name + aliases.
    New,
    /// Matched an existing entity by exact name/alias (case-insensitive).
    Duplicate { entity_name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Added,
    Dismissed,
}

/// Diff a list of LLM-proposed characters against the existing entity DB.
/// - Characters with empty/whitespace names are dropped.
/// - Match is case-insensitive over name + every alias on both sides.
pub fn diff_against_entities(raw: RawCharacters, db: &Entities) -> Vec<ProposedCharacter> {
    let mut out: Vec<ProposedCharacter> = Vec::with_capacity(raw.characters.len());
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();

    for r in raw.characters {
        if r.name.trim().is_empty() {
            continue;
        }
        // Dedupe by canonical name within the LLM response itself; some models
        // list the same character twice with different aliases.
        let canon = r.name.trim().to_lowercase();
        if !seen_lower.insert(canon) {
            continue;
        }

        let proposed_terms: Vec<String> = std::iter::once(r.name.clone())
            .chain(r.aliases.iter().cloned())
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let verdict = db
            .by_id
            .values()
            .filter(|e| e.kind == EntityKind::Character)
            .find_map(|e| {
                let existing_terms: Vec<String> = e
                    .match_terms()
                    .into_iter()
                    .map(|s| s.trim().to_lowercase())
                    .collect();
                if proposed_terms
                    .iter()
                    .any(|p| existing_terms.iter().any(|x| x == p))
                {
                    Some(ProposalVerdict::Duplicate {
                        entity_name: e.name.clone(),
                    })
                } else {
                    None
                }
            })
            .unwrap_or(ProposalVerdict::New);

        out.push(ProposedCharacter {
            raw: r,
            verdict,
            status: ProposalStatus::Pending,
        });
    }
    out
}

/// Turn an accepted proposal into an `Entity` ready for `Entities::save`.
/// `first_seen` is set to the chapter title so the inspector can show
/// where the character was discovered.
pub fn build_entity(p: &RawCharacter, db: &Entities, first_seen: &str) -> Entity {
    let mut id = slugify(&p.name);
    // Avoid clobbering an unrelated existing entity that happens to slugify
    // the same way (different kind, or same kind but the dedup match missed).
    if db.by_id.contains_key(&id) {
        let mut n = 2usize;
        loop {
            let candidate = format!("{id}-{n}");
            if !db.by_id.contains_key(&candidate) {
                id = candidate;
                break;
            }
            n += 1;
        }
    }
    let mut e = Entity::new(EntityKind::Character, id, p.name.trim());
    e.aliases = p
        .aliases
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(p.name.trim()))
        .collect();
    e.role = p.role.trim().to_string();
    e.voice_notes = p.voice_notes.trim().to_string();
    e.first_seen = first_seen.to_string();
    e.tags.push("ai-extracted".to_string());
    e
}

pub const SYSTEM_PROMPT: &str = r#"You are a literary analyst working on an adult urban-fantasy novel.
Extract every named character that appears in the prose below.

Return STRICT JSON in this shape and NOTHING else:

{
  "characters": [
    {
      "name": "<canonical name as written in the prose>",
      "aliases": ["<alternate names or nicknames used in this same prose>"],
      "role": "<short — e.g. 'protagonist', 'guard', 'sister'; empty string if unclear>",
      "voice_notes": "<one sentence on speech style or tone, or empty string>",
      "evidence": "<exact substring from the prose where this character is first mentioned>"
    }
  ]
}

Rules:
- Only include NAMED characters (proper nouns referring to people or sentient beings).
- Do NOT include locations, organizations, ships, deities mentioned but absent, or generic groups.
- Do NOT include pronouns or unnamed characters ("the man", "her father").
- "evidence" MUST be an exact substring of the prose, copyable verbatim.
- Group every alias under one canonical "name". Do not list "Bob" and "Robert" as separate characters if the prose makes clear they are the same person.
- If you are unsure whether a name refers to a character or a place, omit it.
- Maximum 30 characters per response.
- If the prose contains no named characters, return {"characters": []}."#;

pub fn build_user(prose: &str) -> String {
    let mut s = String::new();
    s.push_str("```\n");
    s.push_str(prose);
    if !prose.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::entity::Entity;

    fn db_with(names: &[(&str, &[&str])]) -> Entities {
        let mut db = Entities::default();
        for (i, (name, aliases)) in names.iter().enumerate() {
            let id = format!("e{i}");
            let mut e = Entity::new(EntityKind::Character, &id, *name);
            e.aliases = aliases.iter().map(|s| (*s).to_string()).collect();
            db.by_id.insert(id, e);
        }
        db
    }

    #[test]
    fn parses_minimal_response() {
        let s = r#"```json
        {"characters":[{"name":"Wua","aliases":["the prince"],"role":"protagonist","voice_notes":"clipped","evidence":"Wua looked east."}]}
        ```"#;
        let v = parse_characters(s).expect("parse");
        assert_eq!(v.characters.len(), 1);
        assert_eq!(v.characters[0].name, "Wua");
        assert_eq!(v.characters[0].aliases, vec!["the prince".to_string()]);
    }

    #[test]
    fn diff_marks_duplicates_by_name_or_alias() {
        let db = db_with(&[("Wua", &["the prince"]), ("Yara", &[])]);
        let raw = RawCharacters {
            characters: vec![
                RawCharacter {
                    name: "Wua".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
                RawCharacter {
                    name: "Some New One".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
                RawCharacter {
                    // alias-side match against existing "Wua" / "the prince"
                    name: "The Prince".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
            ],
        };
        let proposals = diff_against_entities(raw, &db);
        assert_eq!(proposals.len(), 3);
        assert!(matches!(
            proposals[0].verdict,
            ProposalVerdict::Duplicate { .. }
        ));
        assert!(matches!(proposals[1].verdict, ProposalVerdict::New));
        assert!(matches!(
            proposals[2].verdict,
            ProposalVerdict::Duplicate { .. }
        ));
    }

    #[test]
    fn diff_drops_blank_names_and_dedupes_within_response() {
        let db = Entities::default();
        let raw = RawCharacters {
            characters: vec![
                RawCharacter {
                    name: "  ".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
                RawCharacter {
                    name: "Yara".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
                RawCharacter {
                    name: "yara".into(),
                    aliases: vec![],
                    role: "".into(),
                    voice_notes: "".into(),
                    evidence: "".into(),
                },
            ],
        };
        let proposals = diff_against_entities(raw, &db);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].raw.name, "Yara");
    }

    #[test]
    fn build_entity_strips_self_alias_and_picks_unique_id() {
        let mut db = Entities::default();
        let existing = Entity::new(EntityKind::Character, "yara", "Yara");
        db.by_id.insert("yara".into(), existing);

        let raw = RawCharacter {
            name: "Yara".into(),
            aliases: vec!["Yara".into(), "Y.".into()],
            role: "sister".into(),
            voice_notes: "wry".into(),
            evidence: "".into(),
        };
        let e = build_entity(&raw, &db, "Ancient/000_Arrival");
        assert_ne!(e.id, "yara"); // disambiguated
        assert_eq!(e.aliases, vec!["Y.".to_string()]);
        assert_eq!(e.first_seen, "Ancient/000_Arrival");
        assert!(e.tags.iter().any(|t| t == "ai-extracted"));
    }
}
