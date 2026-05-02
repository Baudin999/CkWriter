use crate::book::entity::Entity;
use crate::book::Book;
use crate::scope;

/// Build the system prompt for the conversational-AI panel. Anchors the model
/// to the current chapter prose and any in-scope cast so replies don't drift
/// into generic feedback.
pub fn build_system(book: &Book, in_scope: &[&Entity], chapter_label: &str, prose: &str) -> String {
    let mut s = String::new();

    if !book.voice_prompt.trim().is_empty() {
        s.push_str(&book.voice_prompt);
        s.push_str("\n\n---\n\n");
    } else {
        s.push_str(
            "You are a thoughtful writing collaborator for an adult \
             urban-fantasy novel. Discuss the chapter with the author. \
             Protect their voice. Do not invent worldbuilding or characters \
             that aren't already in evidence.\n\n",
        );
    }

    if !book.roadmap.trim().is_empty() {
        s.push_str("## Roadmap (where the story is going)\n\n");
        s.push_str(&scope::tail(&book.roadmap, 2000));
        s.push_str("\n\n---\n\n");
    }

    if !in_scope.is_empty() {
        s.push_str("## Characters in this chapter\n\n");
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

    s.push_str("## Chapter\n\n");
    s.push_str(&format!("Title: {chapter_label}\n\n"));
    s.push_str("```\n");
    s.push_str(prose);
    if !prose.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n\n");

    s.push_str(
        "## How to converse\n\n\
         - Answer the author's questions directly. Be specific. Cite the prose.\n\
         - When asked for a critique, point at concrete passages — quote a phrase, \
           explain what's working or not, and only suggest a change if asked.\n\
         - Never rewrite a paragraph wholesale unless the author explicitly asks.\n\
         - If a question can't be answered from the chapter, say so plainly.\n\
         - Plain prose only. No JSON, no markdown headings unless they help.\n",
    );

    s
}
