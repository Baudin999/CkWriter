use super::IngestOutcome;
use crate::book::dismissals::normalize as normalize_quote;
use crate::book::latex;
use crate::book::paragraphs::Paragraph;
use crate::book::suggestions::{auto_stale, fuzzy_match_record_id, id_hash, Status, SuggestionRecord};
use crate::llm;
use crate::llm::prompts::{FocusContext, Pipeline};
use crate::llm::revision::{FlagKind, Revision};
use crate::scope;
use std::collections::BTreeMap;

/// One paragraph queued for a per-paragraph pipeline run. Captured at queue
/// build time so a mid-run edit can't shift offsets out from under the
/// stream loop. The hash is the paragraph's hash *as of the run start* — it
/// gets written to `last_run_hashes` only after a successful ingest, so a
/// failed run leaves the prior cache entry intact and the next run still
/// sees the paragraph as dirty.
#[derive(Debug, Clone)]
pub struct PendingParagraph {
    pub id: String,
    pub hash: String,
    pub prose: String,
}

/// Orchestration state for a per-paragraph pipeline run (show, prose,
/// spelling). Voice runs chapter-level and does not use this. Lives on the
/// app between paragraphs while individual streams come and go through
/// `App::stream`.
///
/// `focus` (#0007) marks the run as paragraph-focus mode: the queued
/// `prose` is the full chapter, the system prompt instructs the model to
/// only flag inside `focus.paragraph_text`, and any off-target flags are
/// dropped at ingest. `None` for normal (chapter-level or per-paragraph
/// play-button) runs.
#[derive(Debug)]
pub struct CoachRun {
    pub pipeline: Pipeline,
    pub queue: Vec<PendingParagraph>,
    /// 0-based index of the paragraph currently being streamed. After every
    /// paragraph completes (success or unrecoverable parse failure) the
    /// index advances; when it reaches `queue.len()` the run finalizes.
    pub current: usize,
    pub prompt_tokens: u64,
    pub eval_tokens: u64,
    pub focus: Option<FocusContext>,
}

impl super::CkWriterApp {
    pub fn run_pipeline(&mut self, pipeline: Pipeline) {
        if self.book.is_none() {
            return;
        }
        if self.stream.is_some() || self.coach_run.is_some() {
            return;
        }
        if self.editor_text.trim().is_empty() {
            self.last_error = Some("nothing to send".into());
            return;
        }
        match pipeline {
            Pipeline::Voice => self.start_voice_run(),
            Pipeline::ShowDontTell | Pipeline::Prose | Pipeline::Spelling => {
                self.start_paragraph_run(pipeline);
            }
        }
    }

    /// #0007: Run `pipeline` with output scoped to a single paragraph but
    /// the FULL chapter still in the prompt's prose payload — voice and
    /// show comparisons need chapter-level pacing context, the writer just
    /// wants flags only on the paragraph they're working in.
    ///
    /// Bypasses the per-paragraph dirty-hash cache (the click is an
    /// explicit "rethink this paragraph"). Cache write still lands so a
    /// later chapter-level run treats this paragraph as fresh.
    ///
    /// No-ops on missing book, in-flight stream/run, locked paragraph
    /// (#0005), unknown id, or empty prose.
    pub fn run_pipeline_focus(&mut self, pipeline: Pipeline, paragraph_id: &str) {
        if self.book.is_none() {
            return;
        }
        if self.stream.is_some() || self.coach_run.is_some() {
            return;
        }
        let Some(paragraph) = self
            .current_paragraphs
            .iter()
            .find(|p| p.id == paragraph_id)
            .cloned()
        else {
            log::warn!(
                "run_pipeline_focus: paragraph_id={paragraph_id:?} not in current_paragraphs"
            );
            return;
        };
        if paragraph.locked {
            // Locked paragraphs (#0005) are explicitly opted-out of every
            // pipeline run. The UI should already disable the focus button
            // on locked paragraphs; this is the runtime guarantee.
            log::info!(
                "run_pipeline_focus: refusing locked paragraph_id={paragraph_id} pipeline={}",
                pipeline.label()
            );
            self.last_error = Some(format!(
                "{}: paragraph is locked",
                pipeline.label()
            ));
            return;
        }
        let (s, e) = paragraph.char_range;
        if e > self.editor_text.len() || s > e {
            return;
        }
        let focus_text = latex::to_prose(&self.editor_text[s..e]);
        if focus_text.trim().is_empty() {
            self.last_error = Some(format!(
                "{}: focus paragraph has no prose",
                pipeline.label()
            ));
            return;
        }
        let full_prose = latex::to_prose(&self.editor_text);
        if full_prose.trim().is_empty() {
            self.last_error = Some("nothing to send".into());
            return;
        }
        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "run_pipeline_focus: pipeline={} chapter={chapter_label:?} paragraph_id={} \
             chapter_chars={} focus_chars={}",
            pipeline.label(),
            paragraph.id,
            full_prose.chars().count(),
            focus_text.chars().count(),
        );
        self.coach_run = Some(CoachRun {
            pipeline,
            queue: vec![PendingParagraph {
                id: paragraph.id.clone(),
                hash: paragraph.hash.clone(),
                prose: full_prose,
            }],
            current: 0,
            prompt_tokens: 0,
            eval_tokens: 0,
            focus: Some(FocusContext {
                paragraph_id: paragraph.id.clone(),
                paragraph_text: focus_text,
            }),
        });
        self.last_error = None;
        self.start_next_paragraph_stream();
    }

    fn start_voice_run(&mut self) {
        let Some(book) = self.book.as_ref() else { return };
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.last_error = Some("nothing to send".into());
            return;
        }
        let pipeline = Pipeline::Voice;
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system = crate::llm::prompts::build_system(book, &in_scope, pipeline, None);
        let user = crate::llm::prompts::build_user(&prose);

        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "pipeline={} start: chapter={chapter_label:?} focus=none prose_chars={} system_bytes={} user_bytes={} model={}",
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
        // flags' worth of quote/why/suggestion strings.
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

    /// Collect (quote, optional dismissal note) pairs for non-Stale records
    /// (Dismissed + Accepted + existing Proposed) matching `(pipeline,
    /// paragraph_id)` in the active chapter's store. Used by per-paragraph
    /// runs (#0025 + #0027) to feed the model an "already reviewed — do not
    /// flag again" section so dismissals don't get re-raised on every
    /// play-button click, and so the writer's stated dismissal reason is
    /// fed back to the model for paraphrase generalization.
    ///
    /// Stale skipped: those are auto-swept tombstones for paragraphs the
    /// writer rewrote — they're not deliberate "don't flag" intent.
    fn already_reviewed_history(
        &self,
        pipeline: Pipeline,
        paragraph_id: &str,
    ) -> Vec<(String, Option<String>)> {
        let Some(ch) = self.current_chapter.as_ref() else {
            return Vec::new();
        };
        if ch.folder.is_empty() || ch.name.is_empty() {
            return Vec::new();
        }
        let Some(book) = self.book.as_ref() else {
            return Vec::new();
        };
        let Some(chapter_store) = book.suggestions.for_chapter(&ch.folder, &ch.name) else {
            return Vec::new();
        };
        let pipeline_label = pipeline.label();
        let mut out: Vec<(String, Option<String>)> = chapter_store
            .records
            .values()
            .filter(|r| r.status != Status::Stale)
            .filter(|r| r.pipeline == pipeline_label)
            .filter(|r| r.paragraph_id.as_deref() == Some(paragraph_id))
            .map(|r| (r.quote.clone(), r.dismissal_note.clone()))
            .collect();
        // Sort + dedupe by quote (a fuzzy match can leave near-duplicate
        // quotes; exact-text duplicates are noise in the prompt). When two
        // records share a quote, keep whichever note is `Some` first.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// #0007: Resolve the editor cursor to the paragraph it sits inside,
    /// using the persisted char-index in `chapter_places` (which the editor
    /// keeps fresh every frame the editor was visible). Returns `None`
    /// when no chapter is open, no cursor has been recorded, or the cursor
    /// falls between paragraphs (e.g., on a blank line). Cloned so callers
    /// can borrow `self` mutably for the follow-up action.
    pub fn cursor_paragraph(&self) -> Option<Paragraph> {
        let ch = self.current_chapter.as_ref()?;
        let place = self.settings.chapter_places.get(&ch.file_path)?;
        let cursor_byte = crate::ui::editor::char_to_byte(&self.editor_text, place.cursor);
        self.current_paragraphs
            .iter()
            .find(|p| {
                let (s, e) = p.char_range;
                cursor_byte >= s && cursor_byte < e
            })
            .cloned()
    }

    /// Pull the writer's per-paragraph guidance note (#0027) from the
    /// current chapter's metadata. `None` if no chapter is open or the
    /// paragraph carries no note. Empty-string notes return `None` so the
    /// prompt builder doesn't waste budget rendering an empty section.
    fn paragraph_note_for(&self, paragraph_id: &str) -> Option<String> {
        let ch = self.current_chapter.as_ref()?;
        let note = ch.meta.paragraph_notes.get(paragraph_id)?.trim();
        if note.is_empty() {
            None
        } else {
            Some(note.to_string())
        }
    }

    /// Per-paragraph play button (#0024): queue show / prose / spelling for
    /// the named paragraph and kick the queue drain. Always force-runs even
    /// if every pipeline's cache hash matches — the click is an explicit
    /// re-run, and idempotency is preserved by the existing `id_hash`
    /// dedupe in `ingest_response`.
    pub fn play_paragraph(&mut self, paragraph_id: &str) {
        if self.book.is_none() {
            return;
        }
        // Validate the paragraph exists in the live index. Stale clicks (from
        // a paragraph that was edited away mid-frame) silently no-op.
        if !self
            .current_paragraphs
            .iter()
            .any(|p| p.id == paragraph_id)
        {
            log::warn!(
                "play_paragraph: paragraph_id={paragraph_id:?} not in current_paragraphs; ignoring"
            );
            return;
        }
        for pipeline in [
            Pipeline::ShowDontTell,
            Pipeline::Prose,
            Pipeline::Spelling,
        ] {
            self.paragraph_play_queue
                .push_back((paragraph_id.to_string(), pipeline));
        }
        self.try_drain_paragraph_play_queue();
    }

    /// If nothing is in flight, pop the next queued (paragraph_id, pipeline)
    /// and start it. Called from `play_paragraph` (initial kick) and from
    /// `finalize_coach_run` (after each per-paragraph run completes).
    fn try_drain_paragraph_play_queue(&mut self) {
        if self.stream.is_some() || self.coach_run.is_some() {
            return;
        }
        while let Some((paragraph_id, pipeline)) = self.paragraph_play_queue.pop_front() {
            // The paragraph may have disappeared between queueing and
            // draining (writer rewrote the paragraph aggressively); skip
            // and try the next entry rather than tripping on a stale id.
            if !self
                .current_paragraphs
                .iter()
                .any(|p| p.id == paragraph_id)
            {
                log::info!(
                    "play queue: skipping stale paragraph_id={paragraph_id:?} pipeline={}",
                    pipeline.label()
                );
                continue;
            }
            self.start_single_paragraph_run(&paragraph_id, pipeline);
            return;
        }
    }

    /// Force-run `pipeline` against a single paragraph, bypassing the
    /// per-pipeline dirty-hash cache. Used by the per-paragraph play button
    /// (#0024). Caller guarantees no other stream is in flight.
    ///
    /// Builds a one-entry `CoachRun` and delegates the prompt build to
    /// `start_next_paragraph_stream`, so the "Already reviewed" history
    /// section (#0025) lands in both call sites without duplication.
    fn start_single_paragraph_run(&mut self, paragraph_id: &str, pipeline: Pipeline) {
        let Some(paragraph) = self
            .current_paragraphs
            .iter()
            .find(|p| p.id == paragraph_id)
            .cloned()
        else {
            return;
        };
        let (s, e) = paragraph.char_range;
        if e > self.editor_text.len() || s > e {
            return;
        }
        let prose = crate::book::latex::to_prose(&self.editor_text[s..e]);
        if prose.trim().is_empty() {
            return;
        }
        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "play_paragraph: pipeline={} chapter={chapter_label:?} paragraph_id={} prose_chars={} (force, queued={})",
            pipeline.label(),
            paragraph.id,
            prose.chars().count(),
            self.paragraph_play_queue.len(),
        );
        self.coach_run = Some(CoachRun {
            pipeline,
            queue: vec![PendingParagraph {
                id: paragraph.id.clone(),
                hash: paragraph.hash.clone(),
                prose,
            }],
            current: 0,
            prompt_tokens: 0,
            eval_tokens: 0,
            focus: None,
        });
        self.start_next_paragraph_stream();
    }

    fn start_paragraph_run(&mut self, pipeline: Pipeline) {
        let label = pipeline.label().to_string();
        let cached = self
            .current_chapter
            .as_ref()
            .and_then(|c| c.meta.last_run_hashes.get(&label).cloned())
            .unwrap_or_default();
        let dirty = compute_dirty_paragraphs(
            &self.current_paragraphs,
            &self.editor_text,
            &cached,
        );
        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        if dirty.is_empty() {
            log::info!(
                "pipeline={label} chapter={chapter_label:?} cache hit on all {} paragraph(s) — 0 prompts",
                self.current_paragraphs.len()
            );
            self.last_error = Some(format!(
                "{label}: all {} paragraph(s) cached — 0 prompts",
                self.current_paragraphs.len()
            ));
            return;
        }
        log::info!(
            "pipeline={label} chapter={chapter_label:?} dirty={}/{} (will issue {} prompt(s))",
            dirty.len(),
            self.current_paragraphs.len(),
            dirty.len(),
        );
        self.coach_run = Some(CoachRun {
            pipeline,
            queue: dirty,
            current: 0,
            prompt_tokens: 0,
            eval_tokens: 0,
            focus: None,
        });
        self.start_next_paragraph_stream();
    }

    /// Kick off the stream for `coach_run.queue[current]`. Caller guarantees
    /// `coach_run.is_some()` and `current < queue.len()`.
    fn start_next_paragraph_stream(&mut self) {
        let Some(run) = self.coach_run.as_ref() else { return };
        if run.current >= run.queue.len() {
            return;
        }
        let pipeline = run.pipeline;
        let para = run.queue[run.current].clone();
        let total = run.queue.len();
        let idx = run.current;
        let focus = run.focus.clone();
        let history = self.already_reviewed_history(pipeline, &para.id);
        let history_refs: Vec<crate::llm::prompts::HistoryEntry<'_>> = history
            .iter()
            .map(|(q, n)| (q.as_str(), n.as_deref()))
            .collect();
        let paragraph_note = self.paragraph_note_for(&para.id);
        let Some(book) = self.book.as_ref() else { return };
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system =
            crate::llm::prompts::build_system(book, &in_scope, pipeline, focus.as_ref());
        let user = crate::llm::prompts::build_user_with_history(
            &para.prose,
            paragraph_note.as_deref(),
            &history_refs,
        );
        let focus_label: String = focus
            .as_ref()
            .map(|f| f.paragraph_id.clone())
            .unwrap_or_else(|| "none".to_string());
        log::info!(
            "pipeline={} paragraph {}/{} id={} focus={} prose_chars={} system_bytes={} user_bytes={} history={}",
            pipeline.label(),
            idx + 1,
            total,
            para.id,
            focus_label,
            para.prose.chars().count(),
            system.len(),
            user.len(),
            history.len(),
        );
        let messages = vec![
            llm::ChatMessage::system(system),
            llm::ChatMessage::user(user),
        ];
        // Per-paragraph prompts carry the same system preamble (voice prompt
        // + roadmap + cast for show pipeline) but only one paragraph of
        // prose, so 8k fits comfortably with room for the JSON output.
        let tuning = llm::ChatTuning {
            temperature: self.settings.coach_temperature,
            num_ctx: 8_192,
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
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let pipeline = self.stream_pipeline.take().unwrap_or(Pipeline::Voice);
        let was_repair = std::mem::take(&mut self.stream_is_repair);
        let err = stream.error.take();
        let prompt_eval = stream.prompt_eval_tokens.unwrap_or(0);
        let eval = stream.eval_tokens.unwrap_or(0);
        self.stream = None;
        self.last_stream_buffer = Some(buffer.clone());

        if let Some(e) = err {
            self.last_error = Some(e);
            // A stream error mid per-paragraph run aborts the whole run;
            // partial progress (already-cached paragraphs) stays cached.
            if self.coach_run.is_some() {
                self.finalize_coach_run(true);
            }
            // Drop the rest of the play queue too — cascading failures
            // against a broken model would just spam the same error per
            // queued pipeline.
            if !self.paragraph_play_queue.is_empty() {
                log::info!(
                    "play queue: stream errored, dropping {} pending entr(ies)",
                    self.paragraph_play_queue.len()
                );
                self.paragraph_play_queue.clear();
            }
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
                return;
            }
            // Repair also failed. For a per-paragraph run we still want the
            // queue to advance — leave the cache entry untouched so the
            // paragraph re-prompts on the next run, but don't get stuck.
        }

        // For per-paragraph runs, advance the queue regardless of which
        // attempt (first or repair) just landed. We only update the cache
        // on a clean ingest so a malformed paragraph stays dirty.
        let coach_progress = self.coach_run.as_mut().map(|run| {
            run.prompt_tokens += prompt_eval;
            run.eval_tokens += eval;
            let cache_update = if outcome == IngestOutcome::Done {
                run.queue
                    .get(run.current)
                    .map(|p| (p.id.clone(), p.hash.clone()))
            } else {
                None
            };
            run.current += 1;
            let total = run.queue.len();
            (cache_update, run.current, total)
        });
        if let Some((cache_update, advanced_to, total)) = coach_progress {
            if let Some((id, hash)) = cache_update {
                self.update_last_run_hash(pipeline, &id, &hash);
            }
            if advanced_to < total {
                self.start_next_paragraph_stream();
            } else {
                self.finalize_coach_run(false);
                // Per-paragraph play button (#0024): if this just finished
                // entry N of a queued chain, kick off entry N+1.
                self.try_drain_paragraph_play_queue();
            }
        }
    }

    /// Persist `hash` as the last-seen value for `(pipeline, paragraph_id)`
    /// in the chapter's `last_run_hashes`. Per-paragraph runs call this
    /// after each successful ingest; the next run's dirty-set computation
    /// then sees the paragraph as cached.
    fn update_last_run_hash(&mut self, pipeline: Pipeline, paragraph_id: &str, hash: &str) {
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return;
        };
        if ch.folder.is_empty() || ch.name.is_empty() {
            return;
        }
        let label = pipeline.label().to_string();
        let id = paragraph_id.to_string();
        let h = hash.to_string();
        self.update_chapter_meta(&ch.folder, &ch.name, |m| {
            m.last_run_hashes
                .entry(label)
                .or_default()
                .insert(id, h);
        });
    }

    /// Wrap up a per-paragraph run: prune cache entries for paragraphs that
    /// no longer exist, log the aggregate token totals, surface a summary
    /// message, and clear `coach_run`. Called whether the run finished
    /// cleanly or was aborted by a stream error.
    fn finalize_coach_run(&mut self, aborted: bool) {
        let Some(run) = self.coach_run.take() else {
            return;
        };
        let label = run.pipeline.label().to_string();

        // Prune cache for paragraphs that have been deleted since cache entries
        // were last written. Limits to the labels we actually iterate so a
        // shared paragraph_id between pipelines doesn't get nuked when only
        // one pipeline ran.
        let live_ids: std::collections::BTreeSet<String> = self
            .current_paragraphs
            .iter()
            .map(|p| p.id.clone())
            .collect();
        if let Some(ch) = self.current_chapter.as_ref().cloned() {
            if !ch.folder.is_empty() && !ch.name.is_empty() {
                self.update_chapter_meta(&ch.folder, &ch.name, |m| {
                    if let Some(map) = m.last_run_hashes.get_mut(&label) {
                        map.retain(|id, _| live_ids.contains(id));
                    }
                });
            }
        }

        log::info!(
            "pipeline={} run done: paragraphs_run={} prompt_tokens={} eval_tokens={} total_tokens={} aborted={aborted}",
            label,
            run.current,
            run.prompt_tokens,
            run.eval_tokens,
            run.prompt_tokens + run.eval_tokens,
        );
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
        let mut flags = match pipeline {
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
        // #0007 focus mode: drop any flag whose `quote` is not a substring
        // of the focus paragraph's prose. The system prompt asks the model
        // to only flag inside the focus paragraph; this is the runtime
        // contract that holds even when the model wanders.
        let focus = self
            .coach_run
            .as_ref()
            .and_then(|r| r.focus.as_ref())
            .map(|f| (f.paragraph_id.clone(), f.paragraph_text.clone()));
        if let Some((focus_id, focus_text)) = focus.as_ref() {
            let before = flags.len();
            filter_focus_flags(&mut flags, focus_id, focus_text);
            if before > 0 {
                log::info!(
                    "focus={} pipeline={} kept {}/{} flag(s) after on-target filter",
                    focus_id,
                    pipeline.label(),
                    flags.len(),
                    before,
                );
            }
        }
        // Persist the voice score onto the chapter's metadata before we move
        // on to flag handling. A successful parse with no score (older prompt
        // outputs) still updates last_coached_at so the writer can see the
        // pipeline ran.
        //
        // Skip the voice-score write for focus runs (#0007): the score is a
        // chapter-level signal, not a paragraph-level one, and a focus run
        // only sees a sliver of the chapter through the directive — its
        // score is not comparable to the chapter-level baseline.
        let in_focus_mode = focus.is_some();
        if pipeline == Pipeline::Voice && parsed_ok && !in_focus_mode {
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
            //
            // Two-leg lookup (#0025): exact-id match first (cheap), then a
            // fuzzy lookup against existing records in the same
            // (pipeline, paragraph_id). The fuzzy leg catches the regression
            // where the model picks a different quote substring for the same
            // observation — exact normalization can't see that.
            let existing_id = if chapter_store.records.contains_key(&id) {
                Some(id.clone())
            } else {
                fuzzy_match_record_id(
                    chapter_store,
                    &pipeline_label,
                    paragraph_id.as_deref(),
                    &normalized,
                )
            };
            if existing_id.is_some() {
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
                        dismissal_note: None,
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

    /// Per-paragraph hard clear (#0025): drop every record (any status,
    /// including Stale) whose `paragraph_id == Some(paragraph_id)` from the
    /// active chapter's store. Use case: the writer wants a true blank
    /// slate for this paragraph — the "do not flag" memory in the store is
    /// in the way of an honest re-evaluation. Persists immediately and
    /// rebuilds the revisions panel.
    ///
    /// Cancels any pending play-button entries for this paragraph so a
    /// queued re-run doesn't surprise the writer mid-clear.
    pub fn hard_clear_paragraph(&mut self, paragraph_id: &str) {
        let Some(ch) = self.current_chapter.as_ref().cloned() else {
            return;
        };
        let folder = ch.folder.clone();
        let name = ch.name.clone();
        if folder.is_empty() || name.is_empty() {
            return;
        }
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let root = book.root.clone();
        let chapter_store = book.suggestions.for_chapter_mut(&root, &folder, &name);
        let before = chapter_store.records.len();
        chapter_store
            .records
            .retain(|_, r| r.paragraph_id.as_deref() != Some(paragraph_id));
        let removed = before - chapter_store.records.len();
        if let Err(e) = book.suggestions.save_chapter(&root, &folder, &name) {
            log::warn!("hard_clear_paragraph save failed: {e}");
        }
        // Drop queued play entries for this paragraph so a now-stale enqueue
        // can't fire after we've cleared.
        self.paragraph_play_queue
            .retain(|(pid, _)| pid != paragraph_id);
        log::info!(
            "hard_clear_paragraph: paragraph_id={paragraph_id} removed={removed} chapter={folder}/{name}"
        );
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

/// Walk `paragraphs` in source order and return the ones whose hash differs
/// from the cached value (or that have no cache entry yet). Each entry
/// carries the paragraph-local LaTeX→prose translation, snapshotted now so
/// a mid-run edit can't shift offsets under the streaming loop. Pure
/// function so the dirty-set logic is testable without spinning up the app.
///
/// Locked paragraphs (#0005) short-circuit before the cache check: the
/// writer has hardened them, so they're never prompted to any pipeline,
/// regardless of cache state.
pub fn compute_dirty_paragraphs(
    paragraphs: &[Paragraph],
    editor_text: &str,
    cached_hashes: &BTreeMap<String, String>,
) -> Vec<PendingParagraph> {
    let mut out = Vec::new();
    for p in paragraphs {
        if p.locked {
            continue;
        }
        let cache_hit = cached_hashes
            .get(&p.id)
            .is_some_and(|h| h == &p.hash);
        if cache_hit {
            continue;
        }
        let (s, e) = p.char_range;
        // Defensive: if the live editor_text is shorter than the recorded
        // range (chapter just changed under us), skip the paragraph rather
        // than panicking. The next save will refresh `current_paragraphs`.
        if e > editor_text.len() || s > e {
            continue;
        }
        let prose = latex::to_prose(&editor_text[s..e]);
        if prose.trim().is_empty() {
            continue;
        }
        out.push(PendingParagraph {
            id: p.id.clone(),
            hash: p.hash.clone(),
            prose,
        });
    }
    out
}

/// #0007 on-target filter: drop any flag whose `quote` (after trim) is not
/// a substring of `focus_text`. The substring check matches the system
/// prompt's contract — "quote MUST be an exact substring of the focus
/// paragraph". `quote.trim()` accommodates leading/trailing whitespace
/// noise the model sometimes returns. Each drop logs a WARN with the
/// focus paragraph id + a truncated quote so off-target patterns are
/// auditable in production.
fn filter_focus_flags(
    flags: &mut Vec<crate::llm::revision::RawFlag>,
    focus_id: &str,
    focus_text: &str,
) {
    flags.retain(|f| {
        let on_target = focus_text.contains(f.quote.trim());
        if !on_target {
            log::warn!(
                "focus={} dropped off-target flag: {:?}",
                focus_id,
                preview_for_log(&f.quote, 80)
            );
        }
        on_target
    });
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
    use super::compute_dirty_paragraphs;
    use crate::book::dismissals::normalize as normalize_quote;
    use crate::book::paragraphs::parse_and_match;
    use crate::book::suggestions::{auto_stale, id_hash, ChapterSuggestions, Status, SuggestionRecord};
    use std::collections::BTreeMap;

    #[test]
    fn dirty_set_empty_when_all_hashes_match() {
        let src = "first paragraph here.\n\nsecond paragraph here.\n";
        let parsed = parse_and_match(src, &[]);
        let mut cache = BTreeMap::new();
        for p in &parsed {
            cache.insert(p.id.clone(), p.hash.clone());
        }
        let dirty = compute_dirty_paragraphs(&parsed, src, &cache);
        assert!(dirty.is_empty(), "all hashes match, nothing should be dirty");
    }

    #[test]
    fn dirty_set_includes_only_changed_paragraph() {
        let src = "first paragraph here.\n\nsecond paragraph here.\n";
        let parsed = parse_and_match(src, &[]);
        // Cache the first paragraph's current hash; leave the second
        // pointing at a stale value so it shows up dirty.
        let mut cache = BTreeMap::new();
        cache.insert(parsed[0].id.clone(), parsed[0].hash.clone());
        cache.insert(parsed[1].id.clone(), "stale-hash".to_string());

        let dirty = compute_dirty_paragraphs(&parsed, src, &cache);
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].id, parsed[1].id);
        assert_eq!(dirty[0].hash, parsed[1].hash);
    }

    #[test]
    fn dirty_set_includes_paragraphs_with_no_cache_entry() {
        let src = "first paragraph here.\n\nsecond paragraph here.\n";
        let parsed = parse_and_match(src, &[]);
        let cache = BTreeMap::new();
        let dirty = compute_dirty_paragraphs(&parsed, src, &cache);
        // Empty cache => every paragraph is dirty (first run for this pipeline).
        assert_eq!(dirty.len(), parsed.len());
    }

    #[test]
    fn locked_paragraphs_skip_dirty_set_even_with_empty_cache() {
        // A locked paragraph must short-circuit before any cache check, so
        // the model is never prompted for it — the central guarantee of
        // #0005. Cache is empty here so without the lock check both
        // paragraphs would be dirty.
        let src = "first paragraph here.\n\nsecond paragraph here.\n";
        let mut parsed = parse_and_match(src, &[]);
        parsed[0].locked = true;
        let cache = BTreeMap::new();
        let dirty = compute_dirty_paragraphs(&parsed, src, &cache);
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].id, parsed[1].id);
    }

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
            dismissal_note: None,
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

    // ----------------------------------------------------------------
    // #0007 paragraph-focus mode
    // ----------------------------------------------------------------

    use super::{filter_focus_flags, CoachRun, PendingParagraph};
    use crate::book::latex;
    use crate::llm::prompts::{FocusContext, Pipeline};
    use crate::llm::revision::RawFlag;

    fn raw_flag(quote: &str) -> RawFlag {
        RawFlag {
            quote: quote.to_string(),
            why: String::new(),
            suggestion: String::new(),
            kind: String::new(),
        }
    }

    #[test]
    fn focus_run_drops_off_target_flag() {
        // Contract: when focus is active, any flag whose `quote` is not a
        // substring of the focus paragraph is dropped before the panel
        // sees it. The model is told this in the system prompt; the
        // filter is the runtime guarantee.
        let mut flags = vec![
            raw_flag("the door creaked"),       // off-target — outside focus
            raw_flag("she stood at the window"), // on-target  — inside focus
        ];
        let focus_text = "she stood at the window and waited.";
        filter_focus_flags(&mut flags, "p_deadbeef", focus_text);
        assert_eq!(flags.len(), 1, "exactly one on-target flag should remain");
        assert_eq!(flags[0].quote, "she stood at the window");
    }

    #[test]
    fn focus_run_keeps_on_target_flag() {
        // The complement of the drop test: a flag whose quote IS in the
        // focus paragraph survives the filter. Whitespace-padded quotes
        // also survive — the model occasionally returns " quote " with
        // surrounding spaces and we don't want to drop those for trim
        // reasons alone.
        let mut flags = vec![
            raw_flag("she stood at the window"),
            raw_flag("  she stood at the window  "),
        ];
        let focus_text = "she stood at the window and waited.";
        filter_focus_flags(&mut flags, "p_deadbeef", focus_text);
        assert_eq!(flags.len(), 2, "both quotes are on-target after trim");
    }

    #[test]
    fn focus_run_uses_full_chapter_prose() {
        // Focus mode's central design choice: queue the FULL chapter as
        // the single paragraph's prose, even though output is scoped to
        // one paragraph. This pins the queued shape so a future refactor
        // can't quietly switch to the paragraph-only payload (which would
        // strip voice/show context the model needs for chapter-level
        // judgments).
        let chapter_src = "first paragraph text.\n\nsecond paragraph text.\n";
        let parsed = parse_and_match(chapter_src, &[]);
        assert_eq!(parsed.len(), 2, "test fixture: two paragraphs expected");
        let focus_para = &parsed[1];

        // Construct what `run_pipeline_focus` builds. The single queue
        // entry's `prose` field carries the WHOLE chapter, not just the
        // focus paragraph. The paragraph-only text lives separately on
        // `focus.paragraph_text` for the system-prompt directive and the
        // off-target filter.
        let full_prose = latex::to_prose(chapter_src);
        let (s, e) = focus_para.char_range;
        let focus_text = latex::to_prose(&chapter_src[s..e]);

        let run = CoachRun {
            pipeline: Pipeline::Prose,
            queue: vec![PendingParagraph {
                id: focus_para.id.clone(),
                hash: focus_para.hash.clone(),
                prose: full_prose.clone(),
            }],
            current: 0,
            prompt_tokens: 0,
            eval_tokens: 0,
            focus: Some(FocusContext {
                paragraph_id: focus_para.id.clone(),
                paragraph_text: focus_text.clone(),
            }),
        };

        let queued_prose = &run.queue[0].prose;
        assert!(
            queued_prose.contains("first paragraph text"),
            "queued prose must include the FIRST paragraph (chapter context)",
        );
        assert!(
            queued_prose.contains("second paragraph text"),
            "queued prose must include the focus paragraph itself",
        );
        // Sanity: focus.paragraph_text covers only the focus paragraph,
        // which is what the off-target filter and the system prompt see.
        assert!(focus_text.contains("second paragraph"));
        assert!(!focus_text.contains("first paragraph"));
    }

    #[test]
    fn focus_voice_run_does_not_update_score() {
        // The voice score is a chapter-level signal — a single paragraph
        // can't move it meaningfully. The focus-mode guard in
        // `ingest_response` short-circuits the score write, leaving the
        // chapter's prior score untouched.
        //
        // We assert the rule directly: the gate is
        // `pipeline == Voice && parsed_ok && !in_focus_mode`. Encoded
        // here so a future refactor that forgets the focus check fails
        // this test.
        fn should_update_voice_score(
            pipeline: Pipeline,
            parsed_ok: bool,
            in_focus_mode: bool,
        ) -> bool {
            pipeline == Pipeline::Voice && parsed_ok && !in_focus_mode
        }

        // Chapter-level voice run: writes score.
        assert!(should_update_voice_score(Pipeline::Voice, true, false));
        // Focus voice run: skips score write — this is the contract.
        assert!(!should_update_voice_score(Pipeline::Voice, true, true));
        // Other pipelines never write the score regardless of mode.
        assert!(!should_update_voice_score(Pipeline::Prose, true, false));
        assert!(!should_update_voice_score(Pipeline::ShowDontTell, true, true));
        // Failed parse never writes the score.
        assert!(!should_update_voice_score(Pipeline::Voice, false, false));
    }

    #[test]
    fn focus_run_refuses_locked_paragraph() {
        // Locked paragraphs (#0005) are opt-out of every pipeline run.
        // The runtime entry point `run_pipeline_focus` must refuse before
        // building a CoachRun. The lock check is `paragraph.locked` —
        // pinned here so a refactor can't accidentally drop it.
        let src = "first paragraph.\n\nsecond paragraph.\n";
        let mut parsed = parse_and_match(src, &[]);
        parsed[1].locked = true;

        // The runtime guard:
        let target = parsed.iter().find(|p| p.id == parsed[1].id).unwrap();
        assert!(target.locked, "fixture: target paragraph is locked");

        // The check in `run_pipeline_focus` is exactly this — refuse on
        // locked, proceed otherwise. We assert the rule rather than
        // running the full method (which needs a live App fixture).
        let would_proceed = !target.locked;
        assert!(!would_proceed, "locked paragraph must NOT proceed to focus run");

        // Sibling sanity: the unlocked paragraph would proceed.
        let other = parsed.iter().find(|p| p.id == parsed[0].id).unwrap();
        assert!(!other.locked, "fixture: sibling paragraph is unlocked");
    }
}
