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
}

pub fn build_system(book: &Book, in_scope: &[&Entity], pipeline: Pipeline) -> String {
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
- Skip anything that is already showing."#;

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
- If you have nothing to flag, return {"flags": []}."#;

const SPELLING_INSTRUCTIONS: &str = r#"You are a copy editor. Find spelling, punctuation, and grammar mistakes in the prose below.

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
    let mut s = String::new();
    s.push_str("```\n");
    s.push_str(prose);
    if !prose.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n\nReturn the JSON object now. JSON only.\n");
    s
}
