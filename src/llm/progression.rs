//! Per-chapter character progression: ask the model to read a chapter with
//! one specific character in mind and return a snapshot of their state.
//!
//! Output shape mirrors `book::entity::ProgressionEntry` minus `chapter`,
//! which the caller fills in (we trust the chapter slug coming from the UI
//! more than whatever the model would echo back).

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawProgression {
    #[serde(default)]
    pub tone: String,
    #[serde(default)]
    pub situation: String,
    #[serde(default)]
    pub voice_summary: String,
    #[serde(default)]
    pub notable_changes: String,
}

impl RawProgression {
    /// True when the model says the character isn't really in this chapter
    /// (all four fields blank). The caller should not append empty rows to
    /// the timeline — they're noise.
    pub fn is_empty(&self) -> bool {
        self.tone.trim().is_empty()
            && self.situation.trim().is_empty()
            && self.voice_summary.trim().is_empty()
            && self.notable_changes.trim().is_empty()
    }
}

pub fn parse(buf: &str) -> Option<RawProgression> {
    super::parse::parse_json_object(buf, "progression")
}

pub const SYSTEM_PROMPT: &str = r#"You are a literary analyst tracking how a single named character evolves across the chapters of an adult urban-fantasy novel.

You will be given:
- The character's canonical name and any aliases
- That character's prior voice notes (may be empty)
- The label of the chapter being analysed
- The full prose of the chapter

Read the prose with the named character in mind, and return a single JSON object snapshotting their state in this chapter. Capture only what is explicit or strongly implied in the prose — do not invent dialogue or events.

Return STRICT JSON in this shape and NOTHING else:

{
  "tone": "<one line: their emotional register in this chapter>",
  "situation": "<one or two sentences: what they are physically doing, what they are caught up in>",
  "voice_summary": "<one or two sentences: how their speech sounds — vocabulary, rhythm, what they avoid saying>",
  "notable_changes": "<what is different from the prior voice notes; empty string if nothing has shifted>"
}

Rules:
- If the character does not appear in this chapter, return all four fields as the empty string.
- One JSON object only. No markdown, no code fences, no commentary.
- Quote nothing verbatim — paraphrase concisely.
"#;

pub fn build_user(
    character_name: &str,
    aliases: &[String],
    prior_voice_notes: &str,
    last_snapshot: Option<&LastSnapshot>,
    chapter_label: &str,
    prose: &str,
) -> String {
    let aliases = if aliases.is_empty() {
        "(none)".to_string()
    } else {
        aliases.join(", ")
    };
    let voice = if prior_voice_notes.trim().is_empty() {
        "(none yet)".to_string()
    } else {
        prior_voice_notes.trim().to_string()
    };
    let last = match last_snapshot {
        Some(s) => format!(
            "Prior chapter: {chap}\n  voice_summary: {v}\n  notable_changes: {n}\n",
            chap = s.chapter,
            v = blank_or(&s.voice_summary),
            n = blank_or(&s.notable_changes),
        ),
        None => "Prior chapter: (none — this is the first snapshot)\n".to_string(),
    };

    let mut s = String::new();
    s.push_str(&format!("Character: {character_name}\n"));
    s.push_str(&format!("Aliases: {aliases}\n"));
    s.push_str(&format!("Prior voice notes: {voice}\n"));
    s.push_str(&last);
    s.push_str(&format!("Chapter: {chapter_label}\n"));
    s.push_str("```\n");
    s.push_str(prose);
    if !prose.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n");
    s
}

#[derive(Debug, Clone)]
pub struct LastSnapshot {
    pub chapter: String,
    pub voice_summary: String,
    pub notable_changes: String,
}

fn blank_or(s: &str) -> &str {
    if s.trim().is_empty() {
        "(none)"
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_response() {
        let s = r#"```json
        {"tone":"haunted","situation":"newly exiled, walking east","voice_summary":"clipped, fewer questions","notable_changes":"stops calling himself prince"}
        ```"#;
        let v = parse(s).expect("parse");
        assert_eq!(v.tone, "haunted");
        assert_eq!(v.notable_changes, "stops calling himself prince");
        assert!(!v.is_empty());
    }

    #[test]
    fn empty_response_is_empty() {
        let s = r#"{"tone":"","situation":"","voice_summary":"","notable_changes":""}"#;
        let v = parse(s).expect("parse");
        assert!(v.is_empty());
    }

    #[test]
    fn build_user_handles_no_prior_snapshot() {
        let body = build_user(
            "Wua",
            &["the prince".into()],
            "",
            None,
            "Ancient/003_Storm",
            "He looked east.",
        );
        assert!(body.contains("Character: Wua"));
        assert!(body.contains("Aliases: the prince"));
        assert!(body.contains("Prior voice notes: (none yet)"));
        assert!(body.contains("Prior chapter: (none"));
        assert!(body.contains("He looked east."));
    }

    #[test]
    fn build_user_includes_prior_snapshot_when_given() {
        let body = build_user(
            "Wua",
            &[],
            "speaks in commands",
            Some(&LastSnapshot {
                chapter: "Ancient/002".into(),
                voice_summary: "imperious".into(),
                notable_changes: "".into(),
            }),
            "Ancient/003",
            "...",
        );
        assert!(body.contains("Prior chapter: Ancient/002"));
        assert!(body.contains("voice_summary: imperious"));
        assert!(body.contains("notable_changes: (none)"));
    }
}
