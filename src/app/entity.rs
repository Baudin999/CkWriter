use crate::book::entity::{mirror_diff, slugify, Entity, EntityKind, MirrorOp, Relation};
use crate::book::Book;
use crate::extract::EntityMatcher;
use crate::index::CrossChapterIndex;

impl super::CkWriterApp {
    pub fn create_blank_entity(&mut self, kind: EntityKind) {
        let Some(book) = &mut self.book else { return };
        let mut n = 1usize;
        let mut id;
        loop {
            id = format!("new-{}-{n}", kind_singular_id(kind));
            if book.entity(&id).is_none() {
                break;
            }
            n += 1;
        }
        let e = Entity::new(kind, id.clone(), "New entity".to_string());
        let _ = book.save_entity(e);
        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
        // Route through the request helper so a dirty inspector form for
        // the previous selection isn't silently dropped — the new entity
        // is already on disk regardless of the user's discard choice.
        self.request_select_entity(Some(id));
    }

    pub fn commit_entity_edit(&mut self) {
        // Pull the working copy out of the form (drop the form entirely so
        // the inspector re-seeds against the freshly-saved entity on next
        // render — this is what picks up the slugified id and the dropped
        // dangling relations).
        let Some(form) = self.entity_form.take() else {
            return;
        };
        let mut e = form.draft().clone();
        let Some(book) = &mut self.book else { return };
        // Re-slug if name changed and old id no longer matches.
        let original_id = e.id.clone();
        if e.id.is_empty() {
            e.id = slugify(&e.name);
        }
        if original_id != e.id {
            // remove old file
            if let Some(old) = book.entity(&original_id).cloned() {
                let _ = std::fs::remove_file(old.file_path(&book.root));
                book.entities.by_id.remove(&original_id);
            }
        }

        // Snapshot prior relations so we can mirror inverses on save.
        let prev_relations = book
            .entity(&original_id)
            .map(|p| p.relations.clone())
            .unwrap_or_default();
        // Drop relations whose target no longer exists or that point at self;
        // serde-default + manual edits to book.json could otherwise leave dangling pointers.
        e.relations.retain(|r| {
            let id = r.id.trim();
            !id.is_empty() && id != e.id && book.entity(id).is_some()
        });
        let new_relations = e.relations.clone();
        let saved_id = e.id.clone();

        if let Err(err) = book.save_entity(e) {
            self.last_error = Some(format!("save entity: {err}"));
            return;
        }

        let inverse_fn = |k: &str| book.data.inverse_relation(k);
        let ops = mirror_diff(&prev_relations, &new_relations, &saved_id, inverse_fn);
        for op in ops {
            if let Err(err) = apply_mirror_op(book, &saved_id, op) {
                log::warn!("mirror relation op failed: {err}");
            }
        }

        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
    }

    pub fn rebuild_char_index(&mut self) {
        let (Some(book), Some(matcher)) = (self.book.as_ref(), self.matcher.as_ref()) else {
            self.char_index = None;
            return;
        };
        let start = std::time::Instant::now();
        let chapter_count = book.chapters.len();
        let idx = CrossChapterIndex::build(book, matcher);
        log::info!(
            "char index rebuilt in {:?}: chapters={chapter_count} indexed_entities={} total_occurrences={}",
            start.elapsed(),
            idx.entity_count(),
            idx.total_occurrences_all(),
        );
        self.char_index = Some(idx);
    }
}

fn apply_mirror_op(book: &mut Book, self_id: &str, op: MirrorOp) -> anyhow::Result<()> {
    match op {
        MirrorOp::Add { target, kind } => {
            let Some(mut t) = book.entity(&target).cloned() else {
                log::debug!("mirror Add skipped: target {target:?} not found");
                return Ok(());
            };
            // Idempotent: don't double-add the same (kind, self_id).
            let exists = t
                .relations
                .iter()
                .any(|r| r.kind.eq_ignore_ascii_case(&kind) && r.id.eq_ignore_ascii_case(self_id));
            if exists {
                return Ok(());
            }
            t.relations.push(Relation {
                kind,
                id: self_id.to_string(),
            });
            book.save_entity(t)?;
        }
        MirrorOp::Remove { target, kind } => {
            let Some(mut t) = book.entity(&target).cloned() else {
                return Ok(());
            };
            let before = t.relations.len();
            t.relations.retain(|r| {
                !(r.kind.eq_ignore_ascii_case(&kind) && r.id.eq_ignore_ascii_case(self_id))
            });
            if t.relations.len() != before {
                book.save_entity(t)?;
            }
        }
    }
    Ok(())
}

fn kind_singular_id(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Character => "character",
        EntityKind::Location => "location",
        EntityKind::Event => "event",
        EntityKind::Timeline => "timeline",
    }
}
