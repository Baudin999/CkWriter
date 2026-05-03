use super::IngestOutcome;
use crate::book::dismissals::normalize as normalize_quote;
use crate::book::latex;
use crate::book::paragraphs::Paragraph;
use crate::book::suggestions::{auto_stale, id_hash, Status, SuggestionRecord};
use crate::llm;
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{FlagKind, Revision};
use crate::scope;

impl super::CkWriterApp {
    pub fn run_pipeline(&mut self, pipeline: Pipeline) {
        let Some(book) = &self.book else { return };
        if self.stream.is_some() {
            return;
        }
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.last_error = Some("nothing to send".into());
            return;
        }
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system = crate::llm::prompts::build_system(book, &in_scope, pipeline);
        let user = crate::llm::prompts::build_user(&prose);

        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "pipeline={} start: chapter={chapter_label:?} prose_chars={} system_bytes={} user_bytes={} model={}",
            pipeline.label(),
            prose.chars().count(),
            system.len(),
            user.len(),
            self.settings.model,
        );

        let messages = vec![
            llm::ChatMessage::system(system),
            llm::ChatMessage::user(user),
        ];
        // Full-chapter prose can run 40-90KB; 32k tokens fits the system
        // prompt + chapter without truncation. Output cap covers up to ~8
        // flags' worth of quote/why/suggestion strings. Temperature is
        // user-tunable from the AI panel — lower values reduce invented flags.
        let tuning = llm::ChatTuning {
            temperature: self.settings.coach_temperature,
            num_ctx: 32_768,
            num_predict: 2_048,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.stream = Some(handle);
        self.stream_pipeline = Some(pipeline);
        self.last_error = None;
    }

    pub fn poll_stream(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        let _ = stream.poll();
        if stream.done {
            let buffer = std::mem::take(&mut stream.buffer);
            let pipeline = self.stream_pipeline.take().unwrap_or(Pipeline::Voice);
            let was_repair = std::mem::take(&mut self.stream_is_repair);
            let err = stream.error.take();
            self.stream = None;
            self.last_stream_buffer = Some(buffer.clone());
            if let Some(e) = err {
                self.last_error = Some(e);
                return;
            }
            let outcome = self.ingest_response(pipeline, &buffer);
            if outcome == IngestOutcome::NeedsRepair {
                // Dump the unsalvageable response so we can study the patterns
                // offline and tighten the salvage parser. Failures from the
                // first attempt and the repair attempt land in the same dir
                // with distinct suffixes.
                let suffix = if was_repair {
                    "broken-after-repair"
                } else {
                    "broken"
                };
                dump_unsalvageable(pipeline, &buffer, suffix);
                if !was_repair {
                    // One last attempt: hand the broken text back to the model
                    // and ask it to repair the JSON. Guarded by `was_repair`
                    // so a malformed repair response can't trigger another.
                    self.start_json_repair(pipeline, &buffer);
                }
            }
        }
    }

    fn start_json_repair(&mut self, pipeline: Pipeline, broken: &str) {
        log::warn!(
            "pipeline={} repair: asking model to fix malformed JSON ({} bytes)",
            pipeline.label(),
            broken.len(),
        );
        let system = "You are a JSON repair tool. The input is a JSON document that failed to parse. \
Output only the corrected JSON — no commentary, no code fences, no explanation. \
Preserve every valid array element; drop or fix invalid tokens (stray identifiers, \
missing commas, unbalanced braces) to produce strict, parseable JSON. \
The target shape is `{\"flags\":[{\"kind\":\"...\",\"quote\":\"...\",\"why\":\"...\",\"suggestion\":\"...\"}]}`.";
        let user = format!("Fix this JSON:\n\n{broken}");
        let messages = vec![
            llm::ChatMessage::system(system.to_string()),
            llm::ChatMessage::user(user),
        ];
        // Repair runs on a much smaller payload than the original prompt
        // (just the broken response + a short instruction). Same num_predict
        // ceiling so we don't truncate the corrected output.
        let tuning = llm::ChatTuning {
            temperature: 0.1,
            num_ctx: 16_384,
            num_predict: 2_048,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.stream = Some(handle);
        self.stream_pipeline = Some(pipeline);
        self.stream_is_repair = true;
        self.last_error = Some(format!(
            "{}: response was malformed; asking model to repair…",
            pipeline.label()
        ));
    }

    fn ingest_response(&mut self, pipeline: Pipeline, buffer: &str) -> IngestOutcome {
        use crate::llm::revision::{parse_flags_only, parse_voice};
        let parsed_ok;
        let mut voice_score: Option<i32> = None;
        let flags = match pipeline {
            Pipeline::Voice => match parse_voice(buffer) {
                Some(v) => {
                    parsed_ok = true;
                    voice_score = v.score;
                    v.flags
                }
                None => {
                    parsed_ok = false;
                    Vec::new()
                }
            },
            Pipeline::ShowDontTell | Pipeline::Prose | Pipeline::Spelling => {
                match parse_flags_only(buffer) {
                    Some(v) => {
                        parsed_ok = true;
                        v.flags
                    }
                    None => {
                        parsed_ok = false;
                        Vec::new()
                    }
                }
            }
        };
        // Persist the voice score onto the chapter's metadata before we move
        // on to flag handling. A successful parse with no score (older prompt
        // outputs) still updates last_coached_at so the writer can see the
        // pipeline ran.
        if pipeline == Pipeline::Voice && parsed_ok {
            if let Some(ch) = self.current_chapter.as_ref() {
                let folder = ch.folder.clone();
                let name = ch.name.clone();
                if !folder.is_empty() && !name.is_empty() {
                    let now = now_unix();
                    self.update_chapter_meta(&folder, &name, |m| {
                        if voice_score.is_some() {
                            m.voice_score = voice_score;
                        }
                        m.last_coached_at = Some(now);
                    });
                }
            }
        }
        if !parsed_ok {
            log::warn!(
                "pipeline={} parse failed: response_bytes={} preview={:?}",
                pipeline.label(),
                buffer.len(),
                preview_for_log(buffer, 240),
            );
        }

        let raw_count = flags.len();
        let pipeline_label = pipeline.label().to_string();
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            // No chapter context — nothing to persist. Surface anything that
            // came back so the user isn't confused by an empty panel.
            if !parsed_ok {
                self.last_error = Some(format!("{}: no chapter open", pipeline.label()));
                return IngestOutcome::NeedsRepair;
            }
            return IngestOutcome::Done;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();

        let Some(book) = self.book.as_mut() else {
            return IngestOutcome::Done;
        };
        let root = book.root.clone();

        let mut added = 0usize;
        let mut anchored = 0usize;
        let mut already_seen = 0usize;
        let now = now_unix();

        let chapter_store = book
            .suggestions
            .for_chapter_mut(&root, &folder, &name);
        for f in flags {
            if f.quote.trim().is_empty() {
                continue;
            }
            // Anchor against the original LaTeX source first; fall back via
            // prose-stripped translation. Anchor drives both `paragraph_id`
            // resolution and the panel's editor jump.
            let anchor_in_text = crate::llm::revision::anchor(&self.editor_text, &f.quote)
                .or_else(|| {
                    crate::llm::revision::anchor(&latex::to_prose(&self.editor_text), &f.quote)
                        .and_then(|_| {
                            crate::llm::revision::anchor(&self.editor_text, f.quote.trim())
                        })
                });
            if anchor_in_text.is_some() {
                anchored += 1;
            }
            let paragraph_id = anchor_in_text
                .and_then(|(s, _)| paragraph_id_for_offset(s, &self.current_paragraphs));

            let normalized = normalize_quote(&f.quote);
            if normalized.is_empty() {
                continue;
            }
            let id = id_hash(&pipeline_label, paragraph_id.as_deref(), &normalized);

            let kind = if pipeline == Pipeline::Spelling {
                FlagKind::parse(&f.kind)
            } else {
                FlagKind::Other
            };

            // Identity dedupe: if the same id is already on file we leave its
            // status history untouched. This is the whole reason re-running a
            // pipeline doesn't pile up duplicate cards or override a prior
            // dismissal.
            let already_existed = chapter_store.records.contains_key(&id);
            if already_existed {
                already_seen += 1;
            } else {
                chapter_store.records.insert(
                    id.clone(),
                    SuggestionRecord {
                        id: id.clone(),
                        pipeline: pipeline_label.clone(),
                        kind: kind.label().to_string(),
                        paragraph_id: paragraph_id.clone(),
                        quote: f.quote.clone(),
                        normalized_quote: normalized,
                        why: f.why.clone(),
                        suggestion: f.suggestion.clone(),
                        status: Status::Proposed,
                        created_at: now,
                        resolved_at: None,
                    },
                );
                added += 1;
            }
        }

        // Auto-stale sweep: any Proposed record whose paragraph has been
        // rewritten / removed since it was minted becomes Stale.
        let stale_changed =
            auto_stale(chapter_store, &self.current_paragraphs, &self.editor_text, now);

        if let Err(e) = book.suggestions.save_chapter(&root, &folder, &name) {
            log::warn!("suggestions save failed: {e}");
        }

        let filtered_for_log = self.rebuild_revisions_from_store();
        log::info!(
            "pipeline={} ingested: parsed_ok={parsed_ok} raw_flags={raw_count} added={added} \
             dup={already_seen} anchored={anchored} stale_swept={stale_changed} \
             revisions_in_panel={filtered_for_log} response_bytes={}",
            pipeline.label(),
            buffer.len(),
        );

        if added == 0 && raw_count > 0 && already_seen == raw_count {
            self.last_error = Some(format!(
                "{}: {already_seen} flag(s) already in store (no new suggestions)",
                pipeline.label()
            ));
        } else if added == 0 && raw_count == 0 {
            let msg = if !parsed_ok {
                format!(
                    "{}: JSON parse failed — model likely returned prose, not JSON (see log)",
                    pipeline.label()
                )
            } else {
                format!("{}: model returned 0 flags", pipeline.label())
            };
            self.last_error = Some(msg);
        }
        if parsed_ok {
            IngestOutcome::Done
        } else {
            IngestOutcome::NeedsRepair
        }
    }

    pub fn accept_revision(&mut self, id: u32) {
        let Some(idx) = self.revisions.iter().position(|r| r.id == id) else {
            return;
        };
        let rev = self.revisions[idx].clone();
        let Some((s, e)) = rev.anchor else { return };
        if e > self.editor_text.len() || s > e {
            return;
        }
        if self.selected_revision == Some(id) {
            self.selected_revision = None;
        }
        self.editor_text.replace_range(s..e, &rev.suggestion);
        self.dirty = true;
        self.refresh_entity_hits();
        // Mark the underlying record as Accepted before saving — the chapter
        // save recomputes paragraphs and will run another auto-stale sweep
        // off the post-accept state.
        self.update_suggestion_status(&rev.suggestion_id, Status::Accepted);
        if let Err(e) = self.save_chapter() {
            self.last_error = Some(format!("save: {e}"));
        }
        // save_chapter recomputes current_paragraphs; re-anchor and re-filter.
        self.run_auto_stale();
        self.rebuild_revisions_from_store();
    }

    pub fn dismiss_revision(&mut self, id: u32) {
        let Some(rev) = self.revisions.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        if self.selected_revision == Some(id) {
            self.selected_revision = None;
        }
        // Recording is unconditional: the dismissal is durable intent and
        // doesn't depend on the current panel-visibility toggle.
        self.update_suggestion_status(&rev.suggestion_id, Status::Dismissed);
        // Rebuild reflects the new status under both filter modes:
        //  - filter on: card disappears
        //  - filter off: card stays, but flagged is_dismissed = true so the
        //    panel renders it dimmed with a Restore action
        self.rebuild_revisions_from_store();
    }

    /// Flip a previously Dismissed record back to Proposed so it returns to
    /// the panel as a normal flag. Triggered by clicking a dismissed card in
    /// sealing mode (`coach_filter_dismissed = false`).
    pub fn undismiss_revision(&mut self, id: u32) {
        let Some(rev) = self.revisions.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        self.update_suggestion_status(&rev.suggestion_id, Status::Proposed);
        self.rebuild_revisions_from_store();
    }

    /// Select a revision card and jump the editor to its anchor. Toggling the
    /// same revision off returns nothing to the editor (cursor stays put).
    pub fn select_revision(&mut self, id: u32) {
        if self.selected_revision == Some(id) {
            log::info!("select_revision: deselecting id={id}");
            self.selected_revision = None;
            return;
        }
        let anchor = self
            .revisions
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.anchor);
        log::info!("select_revision: id={id} anchor={anchor:?}");
        self.selected_revision = Some(id);
        if let Some((s, _)) = anchor {
            self.jump_to_anchor(s);
        }
    }

    /// Walk the active chapter's store and rebuild `revisions` from
    /// `Proposed` records (always) plus `Dismissed` records (only when the
    /// panel-visibility toggle is off). Returns the number of revisions in
    /// the panel after the rebuild — caller logging only.
    pub fn rebuild_revisions_from_store(&mut self) -> usize {
        self.revisions.clear();
        if self.selected_revision.is_some() {
            // The selection was an in-memory id about to be invalidated.
            self.selected_revision = None;
        }
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return 0;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        let filter_active = self.settings.coach_filter_dismissed;
        let Some(book) = self.book.as_mut() else {
            return 0;
        };
        let root = book.root.clone();
        let chapter_store = book.suggestions.for_chapter_mut(&root, &folder, &name);
        let recs: Vec<SuggestionRecord> = chapter_store
            .records
            .values()
            .filter(|r| match r.status {
                Status::Proposed => true,
                Status::Dismissed => !filter_active,
                Status::Accepted | Status::Stale => false,
            })
            .cloned()
            .collect();

        for rec in recs {
            let pipeline = Pipeline::from_label(&rec.pipeline).unwrap_or_else(|| {
                log::warn!(
                    "unknown pipeline label {:?} in store; defaulting to voice",
                    rec.pipeline
                );
                Pipeline::Voice
            });
            let kind = FlagKind::parse(&rec.kind);
            // Re-anchor the raw quote in the live editor text. Records that
            // can't be anchored still appear so the writer can see/restore
            // them; they sort to the bottom (anchor = None handled by sort).
            let anchor_in_text = crate::llm::revision::anchor(&self.editor_text, &rec.quote);
            let id = self.next_rev_id;
            self.next_rev_id += 1;
            self.revisions.push(Revision {
                id,
                pipeline,
                kind,
                quote: rec.quote,
                why: rec.why,
                suggestion: rec.suggestion,
                anchor: anchor_in_text,
                suggestion_id: rec.id,
                paragraph_id: rec.paragraph_id,
                is_dismissed: matches!(rec.status, Status::Dismissed),
            });
        }
        self.revisions
            .sort_by_key(|r| r.anchor.map(|(s, _)| s).unwrap_or(usize::MAX));
        self.revisions.len()
    }

    /// Run the auto-stale sweep against the current chapter using the live
    /// `current_paragraphs` and `editor_text`. Persists if anything changed.
    pub fn run_auto_stale(&mut self) {
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let root = book.root.clone();
        let chapter_store = book.suggestions.for_chapter_mut(&root, &folder, &name);
        let changed = auto_stale(
            chapter_store,
            &self.current_paragraphs,
            &self.editor_text,
            now_unix(),
        );
        if changed {
            if let Err(e) = book.suggestions.save_chapter(&root, &folder, &name) {
                log::warn!("auto-stale save failed: {e}");
            }
        }
    }

    /// Mutate a single record's status and persist. No-op if the chapter has
    /// no record under `suggestion_id` (e.g. an in-memory revision survived
    /// a record-level deletion that hasn't been wired up).
    fn update_suggestion_status(&mut self, suggestion_id: &str, new_status: Status) {
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let root = book.root.clone();
        let chapter_store = book.suggestions.for_chapter_mut(&root, &folder, &name);
        let Some(rec) = chapter_store.records.get_mut(suggestion_id) else {
            log::warn!(
                "update_suggestion_status: id {suggestion_id:?} not in chapter store {folder}/{name}"
            );
            return;
        };
        rec.status = new_status;
        rec.resolved_at = match new_status {
            Status::Proposed => None,
            _ => Some(now_unix()),
        };
        if let Err(e) = book.suggestions.save_chapter(&root, &folder, &name) {
            log::warn!("suggestions save failed: {e}");
        }
    }
}

fn paragraph_id_for_offset(byte_offset: usize, paragraphs: &[Paragraph]) -> Option<String> {
    paragraphs
        .iter()
        .find(|p| {
            let (s, e) = p.char_range;
            byte_offset >= s && byte_offset < e
        })
        .map(|p| p.id.clone())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persist a malformed LLM response to `<repo>/test-results/` so we can study
/// the failure pattern offline and harden the salvage parser. Uses the build
/// directory of the binary so dumps land in the source tree, not the user's
/// state dir. Failures here are non-fatal — we just log and move on.
fn dump_unsalvageable(pipeline: Pipeline, buffer: &str, suffix: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-results");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("test-results: cannot create {}: {e}", dir.display());
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{ts}-{}-{suffix}.json", pipeline.label()));
    if let Err(e) = std::fs::write(&path, buffer) {
        log::warn!("test-results: write failed for {}: {e}", path.display());
        return;
    }
    log::info!(
        "test-results: dumped malformed {} response ({} bytes) to {}",
        pipeline.label(),
        buffer.len(),
        path.display()
    );
}

fn preview_for_log(s: &str, max: usize) -> String {
    let escaped: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if escaped.chars().count() <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::book::dismissals::normalize as normalize_quote;
    use crate::book::paragraphs::parse_and_match;
    use crate::book::suggestions::{auto_stale, id_hash, ChapterSuggestions, Status, SuggestionRecord};

    fn rec(
        id: &str,
        pipeline: &str,
        paragraph_id: Option<&str>,
        quote: &str,
        status: Status,
    ) -> SuggestionRecord {
        SuggestionRecord {
            id: id.to_string(),
            pipeline: pipeline.to_string(),
            kind: String::new(),
            paragraph_id: paragraph_id.map(|s| s.to_string()),
            quote: quote.to_string(),
            normalized_quote: normalize_quote(quote),
            why: String::new(),
            suggestion: String::new(),
            status,
            created_at: 1,
            resolved_at: None,
        }
    }

    #[test]
    fn re_ingest_is_idempotent_via_identity_hash() {
        // Same pipeline + paragraph_id + normalized quote => same id, even
        // when the raw quote varies in capitalization or whitespace.
        let q1 = "The Dog ran  fast.";
        let q2 = "the dog ran fast.";
        let id1 = id_hash("voice", Some("p_aaaaaaaa"), &normalize_quote(q1));
        let id2 = id_hash("voice", Some("p_aaaaaaaa"), &normalize_quote(q2));
        assert_eq!(id1, id2);

        let mut store = ChapterSuggestions::default();
        let r = rec(&id1, "voice", Some("p_aaaaaaaa"), q1, Status::Proposed);
        store.records.insert(r.id.clone(), r);
        let already = store.records.contains_key(&id2);
        assert!(already, "second ingest of equivalent quote dedupes");
    }

    #[test]
    fn rehydration_re_anchors_raw_quote_in_live_text() {
        // Simulates the chapter-switch path: a record holds a raw quote, and
        // we re-anchor it in the current editor text. Records that fail to
        // anchor return None — the panel still surfaces them so the writer
        // can dismiss/restore by hand.
        let editor_text = "intro\n\nthe dog ran across the open field.\n\nlater, more prose.\n";
        let r = rec(
            "h1",
            "voice",
            None,
            "the dog ran across the open field",
            Status::Proposed,
        );
        let anchor = crate::llm::revision::anchor(editor_text, &r.quote);
        let (s, e) = anchor.expect("quote should anchor in unmodified editor text");
        assert_eq!(&editor_text[s..e], "the dog ran across the open field");

        // Quote that no longer appears returns None — the panel sorts these
        // to the bottom but still displays them.
        let r2 = rec("h2", "voice", None, "completely different prose", Status::Proposed);
        assert!(crate::llm::revision::anchor(editor_text, &r2.quote).is_none());
    }

    #[test]
    fn undismiss_flips_status_and_clears_resolved_at() {
        // Lifecycle: Proposed -> Dismissed (resolved_at set) -> Proposed
        // (resolved_at cleared). Mirrors the un-dismiss path the panel runs
        // when the writer clicks a dismissed card in sealing mode.
        let mut store = ChapterSuggestions::default();
        let mut r = rec("h1", "voice", None, "a quote", Status::Proposed);
        r.resolved_at = None;
        store.records.insert(r.id.clone(), r);

        // Dismiss
        let id = "h1".to_string();
        {
            let r = store.records.get_mut(&id).unwrap();
            r.status = Status::Dismissed;
            r.resolved_at = Some(123);
        }
        assert_eq!(store.records[&id].status, Status::Dismissed);
        assert_eq!(store.records[&id].resolved_at, Some(123));

        // Un-dismiss
        {
            let r = store.records.get_mut(&id).unwrap();
            r.status = Status::Proposed;
            r.resolved_at = None;
        }
        assert_eq!(store.records[&id].status, Status::Proposed);
        assert_eq!(store.records[&id].resolved_at, None);
    }

    #[test]
    fn auto_stale_then_no_op_when_nothing_proposed() {
        let src = "the dog ran across the open field a few minutes after dawn.\n";
        let parsed = parse_and_match(src, &[]);
        let mut store = ChapterSuggestions::default();
        // Only Accepted/Dismissed/Stale records — sweep should make no changes.
        store.records.insert(
            "a".into(),
            rec("a", "voice", Some(&parsed[0].id), "the dog ran", Status::Accepted),
        );
        store.records.insert(
            "b".into(),
            rec("b", "voice", Some(&parsed[0].id), "the dog ran", Status::Dismissed),
        );
        store.records.insert(
            "c".into(),
            rec("c", "voice", Some(&parsed[0].id), "vanished", Status::Stale),
        );
        let changed = auto_stale(&mut store, &parsed, src, 99);
        assert!(!changed);
    }
}
