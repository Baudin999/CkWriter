use crate::book::latex;
use crate::extract::EntityMatcher;
use crate::llm;
use crate::llm::characters::ProposalStatus;

impl super::CkWriterApp {
    /// Send the current chapter's prose to ollama and ask it to extract every
    /// named character. Result is parsed and diffed against the entity DB
    /// when the stream completes.
    pub fn extract_characters_from_chapter(&mut self) {
        if self.char_stream.is_some() {
            log::debug!("character extraction already in flight, ignoring re-trigger");
            return;
        }
        if self.book.is_none() {
            self.char_extract_error = Some("open a book first".into());
            return;
        }
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.char_extract_error = Some("nothing to extract from".into());
            return;
        }
        let chapter_label = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.as_str())
            .unwrap_or("<no chapter>");
        log::info!(
            "character extraction start: chapter={chapter_label:?} prose_chars={} model={}",
            prose.chars().count(),
            self.settings.model,
        );
        let messages = vec![
            llm::ChatMessage::system(llm::characters::SYSTEM_PROMPT.to_string()),
            llm::ChatMessage::user(llm::characters::build_user(&prose)),
        ];
        // Long chapters can run 90KB+ of prose. 32k tokens fits any chapter
        // we've seen and is well within gemma3's supported window.
        // num_predict is generous because a single chapter can yield up to
        // 30 character entries (name + aliases + role + voice + evidence).
        let tuning = llm::ChatTuning {
            temperature: 0.4,
            num_ctx: 32_768,
            num_predict: 4_096,
        };
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.char_stream = Some(handle);
        self.char_extract_error = None;
        // Don't drop existing proposals until we know the new extraction
        // succeeded; user may still be reviewing them.
    }

    pub fn poll_char_extract_stream(&mut self) {
        let Some(stream) = self.char_stream.as_mut() else {
            return;
        };
        let _ = stream.poll();
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.char_stream = None;
        self.last_char_buffer = Some(buffer.clone());
        if let Some(e) = err {
            log::error!("character extraction stream error: {e}");
            self.char_extract_error = Some(e);
            return;
        }
        let Some(book) = &self.book else { return };
        match llm::characters::parse_characters(&buffer) {
            Some(raw) => {
                let count = raw.characters.len();
                let proposals = llm::characters::diff_against_entities(raw, &book.entities);
                let new_count = proposals
                    .iter()
                    .filter(|p| matches!(p.verdict, llm::characters::ProposalVerdict::New))
                    .count();
                let dup_count = proposals.len() - new_count;
                log::info!(
                    "character extraction parsed: raw={count} proposals={} new={new_count} duplicates={dup_count}",
                    proposals.len()
                );
                if proposals.is_empty() {
                    self.char_extract_error = Some(format!(
                        "extraction returned {count} candidate(s) but none survived dedup"
                    ));
                }
                self.char_proposals = proposals;
            }
            None => {
                log::warn!(
                    "character extraction: could not parse JSON ({} bytes returned)",
                    buffer.len()
                );
                self.char_extract_error =
                    Some("could not parse JSON from model response (see log)".into());
            }
        }
    }

    pub fn accept_char_proposal(&mut self, idx: usize) {
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let Some(p) = self.char_proposals.get_mut(idx) else {
            return;
        };
        if p.status != ProposalStatus::Pending {
            return;
        }
        let first_seen = self
            .current_chapter
            .as_ref()
            .map(|c| c.display_title.clone())
            .unwrap_or_default();
        let entity = llm::characters::build_entity(&p.raw, &book.entities, &first_seen);
        let entity_id = entity.id.clone();
        let entity_name = entity.name.clone();
        if let Err(err) = book.save_entity(entity) {
            log::error!("save proposed character {entity_id:?} ({entity_name:?}) failed: {err}");
            self.last_error = Some(format!("save proposed character: {err}"));
            return;
        }
        log::info!(
            "character proposal accepted: id={entity_id:?} name={entity_name:?} first_seen={first_seen:?}"
        );
        p.status = ProposalStatus::Added;
        self.matcher = Some(EntityMatcher::build(&book.entities));
        self.refresh_entity_hits();
        self.rebuild_char_index();
    }

    pub fn dismiss_char_proposal(&mut self, idx: usize) {
        if let Some(p) = self.char_proposals.get_mut(idx) {
            p.status = ProposalStatus::Dismissed;
        }
    }

    /// Accept every Pending proposal whose verdict is `New`. Duplicates and
    /// already-handled rows are left alone.
    pub fn accept_all_new_char_proposals(&mut self) {
        let indices: Vec<usize> = self
            .char_proposals
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.status == ProposalStatus::Pending
                    && matches!(p.verdict, crate::llm::characters::ProposalVerdict::New)
            })
            .map(|(i, _)| i)
            .collect();
        log::info!(
            "accepting {} new character proposals in bulk",
            indices.len()
        );
        for i in indices {
            self.accept_char_proposal(i);
        }
    }

    pub fn clear_char_proposals(&mut self) {
        self.char_proposals.clear();
        self.char_extract_error = None;
    }
}
