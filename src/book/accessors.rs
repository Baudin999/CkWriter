//! Typed read-only views over the Author Layer + Mentions index. See ticket
//! #0008 §"Typed accessor API".
//!
//! The accessor surface is the *typed interface* the agent and skills reason
//! about: `book.accessors().characters().get("yara")` or
//! `book.accessors().timeline().around("event_storm", 2)`. The underlying
//! storage shape (the god-`Entity` struct) is unchanged in v1 — splitting it
//! per-kind is deferred (see ticket "Out of scope").
//!
//! **Mutation is not in this module.** Every method takes `&self`. Direct
//! author edits go through `Book::save_entity`; agent-originated edits go
//! through the proposal queue (`crate::book::proposals`). The trust-boundary
//! enforcement that prevents skills from calling `Book::save_entity` lives in
//! the orchestrator (#0009) — this module just gives the agent a typed
//! read-only API to reason with.

// First non-test consumer is the orchestrator (#0009); v1 ships the surface
// so later tickets land without schema churn. Tests exercise every method;
// the binary doesn't yet, so cargo flags everything as dead. Allow at module
// scope rather than per-item.
#![allow(dead_code)]

use crate::book::entity::{Entity, EntityKind};
use crate::book::Book;

/// A reference to a paragraph that mentions an entity. v1 is just the
/// paragraph ID; a future revision can carry chapter scope when the
/// extraction skill records it. Wrapped in a struct rather than a bare
/// `String` so adding fields later is non-breaking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParagraphRef {
    pub paragraph_id: String,
}

/// Entry point. Returned by `Book::accessors`. Borrows the book for the
/// lifetime of the accessor — no caching, no copies, no mutation.
pub struct Accessors<'a> {
    book: &'a Book,
}

impl<'a> Accessors<'a> {
    pub fn new(book: &'a Book) -> Self {
        Self { book }
    }

    pub fn characters(&self) -> KindView<'a> {
        self.kind_view(EntityKind::Character)
    }
    pub fn locations(&self) -> KindView<'a> {
        self.kind_view(EntityKind::Location)
    }
    pub fn factions(&self) -> KindView<'a> {
        self.kind_view(EntityKind::Faction)
    }
    pub fn items(&self) -> KindView<'a> {
        self.kind_view(EntityKind::Item)
    }
    pub fn magic_rules(&self) -> KindView<'a> {
        self.kind_view(EntityKind::MagicRule)
    }
    pub fn timeline(&self) -> TimelineView<'a> {
        TimelineView { book: self.book }
    }

    fn kind_view(&self, kind: EntityKind) -> KindView<'a> {
        KindView {
            book: self.book,
            kind,
        }
    }
}

/// Per-kind view over the Author Layer. `mentions` is per-kind for ergonomics
/// only — `mentions.json` does not store kind, the lookup is by entity ID
/// (see ticket #0008 status notes).
pub struct KindView<'a> {
    book: &'a Book,
    kind: EntityKind,
}

impl<'a> KindView<'a> {
    /// Fetch by ID, but only if the stored entity's kind matches this view.
    /// Cross-kind lookups deliberately return `None` so accessor consumers
    /// don't accidentally read a `Location` through `characters().get(...)`.
    pub fn get(&self, id: &str) -> Option<&'a Entity> {
        self.book
            .entities
            .get(id)
            .filter(|e| e.kind == self.kind)
    }

    /// All entities of this kind, sorted by `name`. Stable across calls.
    pub fn list(&self) -> Vec<&'a Entity> {
        self.book.entities_of(self.kind)
    }

    /// Case-insensitive substring match on `name` + every alias. Used by the
    /// agent when it has a string from prose and needs to resolve it to a
    /// canonical entity. Sorted by name like `list`.
    pub fn search(&self, query: &str) -> Vec<&'a Entity> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<&Entity> = self
            .list()
            .into_iter()
            .filter(|e| {
                let mut terms = std::iter::once(e.name.as_str())
                    .chain(e.aliases.iter().map(|s| s.as_str()));
                terms.any(|t| t.to_lowercase().contains(&q))
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Paragraph references that mention this entity, per the
    /// agent-extracted `mentions.json` index. Empty when the index hasn't
    /// been built or this entity has no recorded mentions.
    pub fn mentions(&self, id: &str) -> Vec<ParagraphRef> {
        self.book
            .agent_layer
            .mentions_of(id)
            .into_iter()
            .map(|paragraph_id| ParagraphRef { paragraph_id })
            .collect()
    }
}

/// Timeline-specific view. Adds `range` and `around` over the `when` field;
/// also exposes `get`/`list` so callers can stay on this view without
/// jumping back through `Accessors`.
pub struct TimelineView<'a> {
    book: &'a Book,
}

impl<'a> TimelineView<'a> {
    pub fn get(&self, id: &str) -> Option<&'a Entity> {
        self.book
            .entities
            .get(id)
            .filter(|e| e.kind == EntityKind::Timeline)
    }

    /// All timeline events ordered by `when` (lexical). Ties broken by `id`
    /// for deterministic output across calls.
    pub fn list(&self) -> Vec<&'a Entity> {
        let mut events = self.book.entities_of(EntityKind::Timeline);
        events.sort_by(|a, b| a.when.cmp(&b.when).then_with(|| a.id.cmp(&b.id)));
        events
    }

    /// Inclusive `[from, to]` range over the `when` field, lexically.
    pub fn range(&self, from: &str, to: &str) -> Vec<&'a Entity> {
        self.list()
            .into_iter()
            .filter(|e| e.when.as_str() >= from && e.when.as_str() <= to)
            .collect()
    }

    /// `2n+1` events centred on `id`, clamped at the edges. If `id` isn't a
    /// timeline event, returns an empty vec.
    pub fn around(&self, id: &str, n: usize) -> Vec<&'a Entity> {
        let events = self.list();
        let Some(centre) = events.iter().position(|e| e.id == id) else {
            return Vec::new();
        };
        let start = centre.saturating_sub(n);
        let end = (centre + n + 1).min(events.len());
        events[start..end].to_vec()
    }
}

// ============================================================================
// Compile-time proof that the accessor surface is `&self`-only.
// ============================================================================

/// Calling every accessor through a `&Accessors` borrow. If a future change
/// adds a `&mut self` method anywhere on `Accessors`/`KindView`/
/// `TimelineView`, this stops compiling — the ticket's acceptance criterion
/// for "Accessors exposes no mutation method".
#[cfg(test)]
#[allow(dead_code)]
fn _no_mut(a: &Accessors) {
    let _ = a.characters().get("x");
    let _ = a.characters().list();
    let _ = a.characters().search("x");
    let _ = a.characters().mentions("x");
    let _ = a.locations().list();
    let _ = a.factions().list();
    let _ = a.items().list();
    let _ = a.magic_rules().list();
    let _ = a.timeline().get("x");
    let _ = a.timeline().list();
    let _ = a.timeline().range("a", "z");
    let _ = a.timeline().around("x", 2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::entity::Entity;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tempdir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "ckwriter-accessors-{}-{nanos}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Build a minimal book on disk so `Book::open` succeeds, then mutate
    /// its in-memory entity map for the test. Avoids forcing every test to
    /// scaffold a manuscript + chapter.
    fn book_with(entities: Vec<Entity>) -> Book {
        let root = tempdir();
        std::fs::write(root.join("main.tex"), "\\documentclass{book}\n\\begin{document}\n\\end{document}\n").unwrap();
        let mut book = Book::open(&root).expect("open empty book");
        for e in entities {
            book.entities.by_id.insert(e.id.clone(), e);
        }
        book
    }

    fn ev(id: &str, name: &str, when: &str) -> Entity {
        Entity {
            when: when.into(),
            ..Entity::new(EntityKind::Timeline, id, name)
        }
    }

    fn ch(id: &str, name: &str) -> Entity {
        Entity::new(EntityKind::Character, id, name)
    }

    #[test]
    fn characters_get_and_list_are_sorted_by_name() {
        let book = book_with(vec![
            ch("zara", "Zara"),
            ch("ana", "Ana"),
            ch("mira", "Mira"),
        ]);
        let a = Accessors::new(&book);
        assert_eq!(a.characters().get("ana").map(|e| e.name.as_str()), Some("Ana"));
        let names: Vec<&str> = a.characters().list().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Ana", "Mira", "Zara"]);
    }

    #[test]
    fn cross_kind_get_returns_none() {
        let book = book_with(vec![ev("event_x", "Storm", "1023-04-01")]);
        let a = Accessors::new(&book);
        // event_x is a Timeline event — characters().get must not surface it.
        assert!(a.characters().get("event_x").is_none());
        assert!(a.timeline().get("event_x").is_some());
    }

    #[test]
    fn search_matches_name_and_aliases_case_insensitively() {
        let mut alias_char = ch("yara", "Yara");
        alias_char.aliases = vec!["Y.".into(), "Pup".into()];
        let book = book_with(vec![alias_char, ch("kal", "Kal")]);
        let a = Accessors::new(&book);
        assert_eq!(a.characters().search("YAR").len(), 1);
        assert_eq!(a.characters().search("pup").len(), 1, "alias substring");
        assert_eq!(a.characters().search("zzz").len(), 0);
        assert_eq!(a.characters().search("   ").len(), 0, "blank query short-circuits");
    }

    #[test]
    fn timeline_around_returns_2n_plus_1_at_centre() {
        let book = book_with(vec![
            ev("e1", "First", "0001"),
            ev("e2", "Second", "0002"),
            ev("event_storm", "Storm", "0003"),
            ev("e4", "Fourth", "0004"),
            ev("e5", "Fifth", "0005"),
            ev("e6", "Sixth", "0006"),
            ev("e7", "Seventh", "0007"),
        ]);
        let a = Accessors::new(&book);
        let around = a.timeline().around("event_storm", 2);
        let ids: Vec<&str> = around.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["e1", "e2", "event_storm", "e4", "e5"]);
    }

    #[test]
    fn timeline_around_clamps_at_edges() {
        let book = book_with(vec![
            ev("e1", "First", "0001"),
            ev("e2", "Second", "0002"),
            ev("e3", "Third", "0003"),
        ]);
        let a = Accessors::new(&book);
        // n=2 around the first event — only one event before, two after.
        let around = a.timeline().around("e1", 2);
        let ids: Vec<&str> = around.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["e1", "e2", "e3"]);
    }

    #[test]
    fn timeline_around_unknown_id_returns_empty() {
        let book = book_with(vec![ev("e1", "First", "0001")]);
        let a = Accessors::new(&book);
        assert!(a.timeline().around("nope", 2).is_empty());
    }

    #[test]
    fn timeline_range_inclusive_lexical() {
        let book = book_with(vec![
            ev("e1", "First", "1023-04-01"),
            ev("e2", "Second", "1023-05-15"),
            ev("e3", "Third", "1023-09-20"),
        ]);
        let a = Accessors::new(&book);
        let in_range = a.timeline().range("1023-04-01", "1023-05-15");
        let ids: Vec<&str> = in_range.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["e1", "e2"]);
    }

    #[test]
    fn mentions_round_trip_through_accessor() {
        let book_root = {
            let root = tempdir();
            std::fs::write(root.join("main.tex"), "\\documentclass{book}\n\\begin{document}\n\\end{document}\n").unwrap();
            root
        };
        let mut book = Book::open(&book_root).expect("open book");
        book.entities.by_id.insert("yara".into(), ch("yara", "Yara"));
        book.agent_layer
            .set_mentions("yara", vec!["p_aaaaaaaa".into(), "p_bbbbbbbb".into()])
            .unwrap();

        let a = Accessors::new(&book);
        let refs = a.characters().mentions("yara");
        assert_eq!(
            refs,
            vec![
                ParagraphRef { paragraph_id: "p_aaaaaaaa".into() },
                ParagraphRef { paragraph_id: "p_bbbbbbbb".into() },
            ]
        );
    }
}
