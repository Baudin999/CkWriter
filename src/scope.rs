use crate::book::entity::{Entity, EntityKind};
use crate::book::Book;
use crate::extract::EntityHit;

/// Entities whose voice notes should be injected into the LLM system prompt
/// for the current chapter.
pub fn voice_context_entities<'a>(book: &'a Book, hits: &[EntityHit]) -> Vec<&'a Entity> {
    use std::collections::HashSet;
    let ids: HashSet<&str> = hits
        .iter()
        .filter(|h| h.kind == EntityKind::Character)
        .map(|h| h.entity_id.as_str())
        .collect();
    let mut v: Vec<&Entity> = ids.into_iter().filter_map(|id| book.entity(id)).collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Truncate text to ~`max_chars` keeping the last `max_chars` portion (most-recent context).
pub fn tail(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let skip = text.chars().count() - max_chars;
    text.chars().skip(skip).collect()
}
