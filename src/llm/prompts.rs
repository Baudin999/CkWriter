use crate::book::entity::Entity;
use crate::book::Book;
use crate::scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipeline {
    Voice,
    ShowDontTell,
    Prose,
    Spelling,
}

impl Pipeline {
    pub fn label(self) -> &'static str {
        match self {
            Pipeline::Voice => "voice",
            Pipeline::ShowDontTell => "show, don't tell",
            Pipeline::Prose => "prose",
            Pipeline::Spelling => "spelling",
        }
    }

    /// Reverse of [`Self::label`]; used to rebuild in-memory `Revision`s from
    /// persisted `SuggestionRecord`s on chapter open.
    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "voice" => Pipeline::Voice,
            "show, don't tell" => Pipeline::ShowDontTell,
            "prose" => Pipeline::Prose,
            "spelling" => Pipeline::Spelling,
            _ => return None,
        })
    }
}

/// Per-paragraph scope for focus-mode coaching (#0007). When threaded into
/// [`build_system`], the prose payload still carries the full chapter (so
/// voice / show comparisons keep their context) but the model is instructed
/// to only emit flags that anchor inside `paragraph_text`. The runtime drops
/// any off-target flag the model sneaks past the directive — the prompt
/// suffix is best-effort, the ingest filter is the contract.
#[derive(Debug, Clone)]
pub struct FocusContext {
    pub paragraph_id: String,
    pub paragraph_text: String,
}

pub fn build_system(
    book: &Book,
    in_scope: &[&Entity],
    pipeline: Pipeline,
    focus: Option<&FocusContext>,
) -> String {
    // Mechanics-only pipelines (prose, spelling) get a lean system prompt.
    // The book voice prompt + roadmap + cast preamble used by voice/show was
    // pulling these runs toward freeform critique and breaking JSON mode.
    let needs_book_context = matches!(pipeline, Pipeline::Voice | Pipeline::ShowDontTell);

    let mut s = String::new();

    if needs_book_context {
        if !book.voice_prompt.trim().is_empty() {
            s.push_str(&book.voice_prompt);
            s.push_str("\n\n---\n\n");
        } else {
            s.push_str(
                "You are an editor for an adult urban-fantasy novel. \
                 Protect the author's voice. Flag, don't rewrite. \
                 Never invent worldbuilding or characters.\n\n",
            );
        }

        if !book.roadmap.trim().is_empty() {
            s.push_str("## Roadmap (where the story is going)\n\n");
            s.push_str(&scope::tail(&book.roadmap, 2000));
            s.push_str("\n\n---\n\n");
        }

        if !in_scope.is_empty() {
            s.push_str("## Characters in this scene\n\n");
            for e in in_scope {
                s.push_str(&format!("- **{}**", e.name));
                if !e.role.is_empty() {
                    s.push_str(&format!(" ({})", e.role));
                }
                s.push('\n');
                if !e.tone.is_empty() {
                    s.push_str(&format!("  - tone: {}\n", e.tone));
                }
                if !e.voice_notes.is_empty() {
                    s.push_str(&format!("  - voice: {}\n", e.voice_notes));
                }
            }
            s.push('\n');
        }
    }

    s.push_str(match pipeline {
        Pipeline::Voice => VOICE_INSTRUCTIONS,
        Pipeline::ShowDontTell => SHOW_INSTRUCTIONS,
        Pipeline::Prose => PROSE_INSTRUCTIONS,
        Pipeline::Spelling => SPELLING_INSTRUCTIONS,
    });

    // #0007: paragraph-focus directive. Always appended LAST so prior
    // pipeline instructions stay intact. `focus = None` collapses to a
    // no-op so chapter-level callers stay byte-identical.
    if let Some(fc) = focus {
        s.push_str("\n\n## Focus paragraph\n\n");
        s.push_str(
            "The prose below is the WHOLE chapter — it is included so you can judge \
             voice, pacing, and continuity in context. However, you must ONLY emit \
             flags whose `quote` is an exact substring of the focus paragraph below. \
             Do not flag any prose outside the focus paragraph; off-target flags will \
             be dropped.\n\n",
        );
        s.push_str(&format!("Focus paragraph id: {}\n\n", fc.paragraph_id));
        s.push_str("Focus paragraph text:\n```\n");
        s.push_str(&fc.paragraph_text);
        if !fc.paragraph_text.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("```\n");
    }

    s
}

const VOICE_INSTRUCTIONS: &str = r#"## Task

Review the prose below for **voice match**. Return STRICT JSON in this shape and NOTHING else:

{
  "score": <integer 1-10>,
  "flags": [
    { "quote": "<exact substring from the prose>", "why": "<one sentence>", "suggestion": "<one sentence>" }
  ],
  "preserved": [ "<exact substring the author should keep>" ]
}

Rules:
- "quote" MUST be an exact substring of the prose, copyable verbatim. Otherwise the editor cannot anchor it.
- Maximum 8 flags. Pick the highest-impact issues.
- Do not propose full rewrites. Suggestions are one sentence each.
- Do not invent worldbuilding."#;

const SHOW_INSTRUCTIONS: &str = r#"## Task

Find places where the prose **tells** instead of **shows** an emotion or state.
Return STRICT JSON:

{
  "flags": [
    { "quote": "<exact substring>", "why": "<why it tells>", "suggestion": "<sensory or behavioral substitution, one sentence>" }
  ]
}

Rules:
- "quote" MUST be an exact substring.
- Maximum 6 flags. Highest-impact only.
- Suggestions favor concrete sensory or behavioral detail (smell, sound, gesture, weight, temperature, taste, posture).
- Skip anything that is already showing.
- If the prose is already showing well, return {"flags": []}. Do not invent problems to fill the list. Zero flags is a valid, expected answer when the prose is strong."#;

const PROSE_INSTRUCTIONS: &str = r#"You are a prose-mechanics editor. Critique sentence rhythm, dead verbs, redundancy, adjective pile-ups, and filler hedges in the prose below.

Return STRICT JSON in this exact shape and NOTHING else — no preface, no commentary, no markdown fences, no code blocks. Your entire response must be a single JSON object.

{
  "flags": [
    { "quote": "<exact substring of the prose>", "why": "<one sentence>", "suggestion": "<one-sentence trim or rephrase>" }
  ]
}

Rules:
- "quote" MUST be an exact substring of the prose, copyable verbatim.
- Maximum 8 flags. Prefer surgical cuts over rewrites.
- Only flag genuine mechanical problems. Do not invent issues to fill the list, and do not flag stylistic choices that are working.
- If the prose is already clean, return {"flags": []}. Zero flags is a valid, expected answer."#;

const SPELLING_INSTRUCTIONS: &str = r#"You are a copy editor for US English. Find spelling, punctuation, and grammar mistakes in the prose below.

Use US English spelling and punctuation conventions:
- "color", "honor", "favor", "neighbor" — not "colour", "honour", "favour", "neighbour".
- "realize", "organize", "analyze" — not "realise", "organise", "analyse".
- "traveled", "canceled", "labeled" — not "travelled", "cancelled", "labelled".
- "center", "theater", "meter" — not "centre", "theatre", "metre".
- Serial (Oxford) comma is acceptable; do not flag its presence or absence.
- Place commas and periods inside closing quotation marks.
- Use straight or curly quotes consistently with the surrounding prose; do not flap quotes that match the document's existing style.
- Flag British spellings as spelling mistakes.

Return STRICT JSON in this exact shape and NOTHING else — no preface, no commentary, no markdown fences, no code blocks. Your entire response must be a single JSON object.

{
  "flags": [
    {
      "kind": "spelling" | "punctuation" | "grammar",
      "quote": "<exact substring containing the mistake>",
      "why": "<one short sentence — what kind of mistake>",
      "suggestion": "<the corrected substring, drop-in replacement for quote>"
    }
  ]
}

Rules:
- "kind" MUST be exactly one of: "spelling", "punctuation", "grammar". Pick the dominant category for that mistake.
- "quote" MUST be an exact substring of the prose, copyable verbatim.
- "suggestion" MUST be a drop-in replacement for "quote" — replacing "quote" with "suggestion" must yield correct text.
- Keep "quote" short — just the words around the error, not whole sentences.
- Cover spelling, punctuation, and grammar only. Do NOT rewrite for style, voice, word choice, or rhythm.
- Skip proper nouns, invented words, dialect, and intentional misspellings. When unsure, skip.
- If there are no mistakes, return {"flags": []}."#;

pub fn build_user(prose: &str) -> String {
    build_user_with_history(prose, None, &[])
}

/// One entry in the "Already reviewed — do not flag again" prompt section
/// (#0025 + #0027). `quote` is the text the model previously flagged;
/// `dismissal_note` (when present) is the writer's stated reason for
/// dismissing it, threaded back so the model sees the rationale and can
/// generalize across paraphrases. `None` collapses to "no note rendered".
pub type HistoryEntry<'a> = (&'a str, Option<&'a str>);

/// Like [`build_user`] but with two writer-facing context sections threaded
/// into the prompt:
///
/// - `paragraph_note` (#0027) — the writer's intent for this paragraph,
///   rendered above everything else as `## Author guidance for this
///   paragraph`. The model is asked to treat it as deliberate intent and
///   avoid flagging prose consistent with it.
/// - `history` (#0025 + #0027) — prior Dismissed / Accepted quotes plus
///   their optional dismissal notes, rendered as the "Already reviewed —
///   do not flag again" section so dismissals don't get re-raised on
///   every per-paragraph play-button click.
///
/// Empty `paragraph_note` (`None` or `""`) and empty `history` together
/// collapse to byte-identical output with the legacy `build_user` so
/// chapter-level callers (which carry no per-paragraph context) stay
/// unchanged.
pub fn build_user_with_history(
    prose: &str,
    paragraph_note: Option<&str>,
    history: &[HistoryEntry<'_>],
) -> String {
    let mut s = String::new();
    if let Some(note) = paragraph_note {
        let trimmed = note.trim();
        if !trimmed.is_empty() {
            s.push_str("## Author guidance for this paragraph\n\n");
            s.push_str(trimmed);
            s.push_str(
                "\n\nTreat the guidance above as the author's stated intent for this \
                 paragraph; do NOT flag prose that is consistent with it.\n\n---\n\n",
            );
        }
    }
    if !history.is_empty() {
        s.push_str("## Already reviewed — do not flag again\n\n");
        s.push_str(
            "The author has already seen and resolved these quotes from the prose below. \
             Treat them as deliberate or already addressed; do NOT include them in `flags`. \
             Where a dismissal reason is given, treat it as authoritative — apply the same \
             reasoning to paraphrases of the same complaint.\n\n",
        );
        for (quote, note) in history {
            // One line per quote; truncate aggressively-long quotes so the
            // section can't dominate the prompt budget. The model only needs
            // enough text to recognize the same observation.
            let trimmed = quote.trim();
            if trimmed.is_empty() {
                continue;
            }
            let preview: String = trimmed.chars().take(240).collect();
            s.push_str("- ");
            s.push_str(&preview);
            if trimmed.chars().count() > 240 {
                s.push('…');
            }
            // Append the writer's dismissal note when present and non-empty,
            // matching the "<quote> — dismissed because: <note>" shape from
            // the #0027 ticket.
            if let Some(note_text) = note {
                let note_trimmed = note_text.trim();
                if !note_trimmed.is_empty() {
                    s.push_str(" — dismissed because: ");
                    s.push_str(note_trimmed);
                }
            }
            s.push('\n');
        }
        s.push_str("\n---\n\n");
    }
    s.push_str("```\n");
    s.push_str(prose);
    if !prose.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n\nReturn the JSON object now. JSON only.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::data::BookData;
    use crate::book::entity::Entities;
    use crate::book::manuscript::Manuscript;
    use crate::book::suggestions::SuggestionStore;
    use crate::book::tree::FileNode;
    use crate::book::{Book, BookConfig};
    use std::path::PathBuf;

    /// Minimal `Book` with empty voice prompt, roadmap, and entities — just
    /// enough for `build_system` to produce its non-context-bearing prompt
    /// suffix. Real chapters/manuscripts/entities are not exercised here;
    /// the focus-mode contract is independent of book content.
    fn empty_book() -> Book {
        Book {
            root: PathBuf::new(),
            main_tex: PathBuf::new(),
            chapters: Vec::new(),
            manuscript: Manuscript::default(),
            file_tree: FileNode {
                name: String::new(),
                path: PathBuf::new(),
                is_dir: true,
                children: Vec::new(),
            },
            entities: Entities::default(),
            voice_prompt: String::new(),
            roadmap: String::new(),
            config: BookConfig::default(),
            data: BookData::default(),
            suggestions: SuggestionStore::default(),
        }
    }

    #[test]
    fn build_system_no_focus_byte_identical_to_legacy() {
        // Contract for chapter-level callers (#0007 design): passing
        // `focus = None` must produce a prompt that does NOT include the
        // focus-mode suffix. The chapter-level behavior of every existing
        // pipeline depends on the prompt being identical to the pre-focus
        // version when no focus is set.
        let book = empty_book();
        for pipeline in [
            Pipeline::Voice,
            Pipeline::ShowDontTell,
            Pipeline::Prose,
            Pipeline::Spelling,
        ] {
            let s = build_system(&book, &[], pipeline, None);
            assert!(
                !s.contains("## Focus paragraph"),
                "focus=None must not append the focus directive for pipeline={:?}; got:\n{s}",
                pipeline,
            );
            assert!(
                !s.contains("Focus paragraph id:"),
                "focus=None must not mention a paragraph id; got:\n{s}",
            );
        }
    }

    #[test]
    fn build_system_focus_appends_directive_with_paragraph_text() {
        // With `Some(FocusContext)`, the suffix MUST contain (a) the focus
        // section header, (b) the paragraph id, and (c) the verbatim
        // paragraph text. The text appears inside a fenced block so the
        // model can locate the boundary unambiguously even when the prose
        // contains markdown-like punctuation.
        let book = empty_book();
        let fc = FocusContext {
            paragraph_id: "p_deadbeef".to_string(),
            paragraph_text: "She stood at the door and waited.".to_string(),
        };
        let s = build_system(&book, &[], Pipeline::Prose, Some(&fc));
        assert!(
            s.contains("## Focus paragraph"),
            "focus directive header missing in:\n{s}",
        );
        assert!(
            s.contains("Focus paragraph id: p_deadbeef"),
            "focus paragraph id missing in:\n{s}",
        );
        assert!(
            s.contains("She stood at the door and waited."),
            "focus paragraph text missing in:\n{s}",
        );
        // Suffix lands AFTER the pipeline instructions so the model sees
        // the task definition first, then the constraint. Not the other
        // way around.
        let task_idx = s
            .find("## Task")
            .or_else(|| s.find("You are a prose-mechanics editor"))
            .expect("pipeline instructions present");
        let focus_idx = s.find("## Focus paragraph").unwrap();
        assert!(
            task_idx < focus_idx,
            "focus directive must come AFTER the pipeline task: task={task_idx} focus={focus_idx}",
        );
    }

    #[test]
    fn legacy_no_history_no_note_matches_build_user() {
        // build_user(prose) is build_user_with_history(prose, None, &[]) by
        // construction; the byte-for-byte equivalence is the contract that
        // chapter-level callers rely on. Pin it here so we notice if either
        // function drifts.
        let prose = "the dog ran fast across the open field hunting rabbits.";
        let baseline = build_user(prose);
        let through_history = build_user_with_history(prose, None, &[]);
        assert_eq!(baseline, through_history);
    }

    #[test]
    fn paragraph_note_renders_above_already_reviewed() {
        // Non-empty note must appear in its own section, ABOVE the existing
        // "Already reviewed" block — paragraph guidance is prospective; the
        // dismissal list is reactive, and the model should read intent
        // before it reads corrections.
        let out = build_user_with_history(
            "prose here.",
            Some("this paragraph is supposed to read flat"),
            &[("the cat sat", None)],
        );
        let note_idx = out
            .find("## Author guidance for this paragraph")
            .expect("note section present");
        let history_idx = out
            .find("## Already reviewed — do not flag again")
            .expect("history section present");
        assert!(
            note_idx < history_idx,
            "author-guidance section must precede already-reviewed: note={note_idx} history={history_idx}",
        );
        assert!(out.contains("this paragraph is supposed to read flat"));
    }

    #[test]
    fn empty_paragraph_note_collapses_to_no_section() {
        // An empty (or whitespace-only) note must not render an empty
        // section — that would waste prompt budget and look like an
        // incomplete instruction to the model.
        let out_none = build_user_with_history("prose.", None, &[]);
        let out_empty = build_user_with_history("prose.", Some(""), &[]);
        let out_blank = build_user_with_history("prose.", Some("   \n\t"), &[]);
        assert_eq!(out_none, out_empty);
        assert_eq!(out_none, out_blank);
        assert!(!out_none.contains("Author guidance"));
    }

    #[test]
    fn dismissal_note_renders_alongside_quote() {
        // History entry with a note → "<quote> — dismissed because: <note>".
        // Entry without a note → bare "<quote>". Both shapes must be present
        // when both kinds of records share the section.
        let out = build_user_with_history(
            "prose.",
            None,
            &[
                ("really tired", Some("colloquial register is intentional")),
                ("the cat sat", None),
            ],
        );
        assert!(
            out.contains("- really tired — dismissed because: colloquial register is intentional"),
            "annotated entry missing or malformed in:\n{out}",
        );
        assert!(
            out.contains("- the cat sat\n"),
            "plain entry missing or malformed in:\n{out}",
        );
    }

    #[test]
    fn empty_dismissal_note_collapses_to_bare_quote() {
        // A note that's `Some("")` or all-whitespace must render the same as
        // `None` — the writer has *registered* the record but not annotated
        // it; we don't want a trailing dangling em-dash in the prompt.
        let out_none = build_user_with_history("prose.", None, &[("the cat sat", None)]);
        let out_empty = build_user_with_history("prose.", None, &[("the cat sat", Some(""))]);
        let out_blank = build_user_with_history("prose.", None, &[("the cat sat", Some("  "))]);
        assert_eq!(out_none, out_empty);
        assert_eq!(out_none, out_blank);
        assert!(!out_none.contains("dismissed because"));
    }
}
