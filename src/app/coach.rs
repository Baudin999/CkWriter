use super::IngestOutcome;
use crate::book::latex;
use crate::llm;
use crate::llm::prompts::Pipeline;
use crate::llm::revision::Revision;
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
        // flags' worth of quote/why/suggestion strings.
        let tuning = llm::ChatTuning {
            temperature: 0.4,
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
        use crate::llm::revision::{anchor, parse_flags_only, parse_voice, FlagKind};
        let prose = latex::to_prose(&self.editor_text);
        let parsed_ok;
        let flags = match pipeline {
            Pipeline::Voice => match parse_voice(buffer) {
                Some(v) => {
                    parsed_ok = true;
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
        if !parsed_ok {
            log::warn!(
                "pipeline={} parse failed: response_bytes={} preview={:?}",
                pipeline.label(),
                buffer.len(),
                preview_for_log(buffer, 240),
            );
        }

        let raw_count = flags.len();
        let mut added = 0usize;
        let mut anchored = 0usize;
        for f in flags {
            if f.quote.trim().is_empty() {
                continue;
            }
            // Try anchoring against the original LaTeX text first; fall back to
            // anchor in the prose-stripped string and translate by string search.
            let anchor_in_text = anchor(&self.editor_text, &f.quote).or_else(|| {
                anchor(&prose, &f.quote).and_then(|_| anchor(&self.editor_text, f.quote.trim()))
            });
            if anchor_in_text.is_some() {
                anchored += 1;
            }

            // Only the spelling pipeline ships per-flag categories;
            // voice/show/prose collapse to FlagKind::Other and are coloured
            // by their pipeline instead.
            let kind = if pipeline == Pipeline::Spelling {
                FlagKind::parse(&f.kind)
            } else {
                FlagKind::Other
            };

            let id = self.next_rev_id;
            self.next_rev_id += 1;
            self.revisions.push(Revision {
                id,
                pipeline,
                kind,
                quote: f.quote.clone(),
                why: f.why.clone(),
                suggestion: f.suggestion.clone(),
                anchor: anchor_in_text,
            });
            added += 1;
        }
        // Sort by position in the source so the cards read in reading order;
        // unanchored flags sink to the bottom. Stable sort preserves id order
        // among same-position ties.
        self.revisions
            .sort_by_key(|r| r.anchor.map(|(s, _)| s).unwrap_or(usize::MAX));
        log::info!(
            "pipeline={} ingested: parsed_ok={parsed_ok} raw_flags={raw_count} added={added} anchored={anchored} response_bytes={}",
            pipeline.label(),
            buffer.len(),
        );
        if added == 0 {
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
        // Drop the accepted revision and shift remaining anchors past the
        // replacement point. Overlapping anchors lose their position because
        // the underlying span no longer exists.
        let delta = rev.suggestion.len() as isize - (e - s) as isize;
        self.revisions.remove(idx);
        for r in &mut self.revisions {
            if let Some((rs, re)) = r.anchor {
                if rs >= e {
                    let new_s = (rs as isize + delta).max(0) as usize;
                    let new_e = (re as isize + delta).max(0) as usize;
                    r.anchor = Some((new_s, new_e));
                } else if re > s {
                    r.anchor = None;
                }
            }
        }
        self.refresh_entity_hits();
        if let Err(e) = self.save_chapter() {
            self.last_error = Some(format!("save: {e}"));
        }
    }

    pub fn dismiss_revision(&mut self, id: u32) {
        self.revisions.retain(|r| r.id != id);
        if self.selected_revision == Some(id) {
            self.selected_revision = None;
        }
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
