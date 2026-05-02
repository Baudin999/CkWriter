use crate::book::latex;
use crate::llm;

impl super::CkWriterApp {
    /// Kick off the per-character progression run for the selected character
    /// against the current chapter. Auto-commits the entity dirty buffer first
    /// if it targets the same character — otherwise the AI append would be
    /// clobbered when the user later clicks Save on a stale form.
    pub fn track_progression_for(&mut self, entity_id: &str) {
        if self.progression_stream.is_some() {
            self.progression_status = Some("progression run already in flight".into());
            return;
        }
        if !self.ollama_ok {
            self.progression_status = Some("ollama unreachable".into());
            return;
        }
        let Some(ch) = self.current_chapter.clone() else {
            self.progression_status = Some("open a chapter first".into());
            return;
        };
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.progression_status = Some("nothing to analyse".into());
            return;
        }

        // entity_dirty is always populated while the inspector is open; only
        // auto-commit when the working copy actually differs from disk.
        let needs_commit = match (self.entity_dirty.as_ref(), self.book.as_ref()) {
            (Some(d), Some(book)) if d.id == entity_id => book
                .entity(entity_id)
                .map(|saved| saved != d)
                .unwrap_or(true),
            _ => false,
        };
        if needs_commit {
            self.commit_entity_edit();
        }

        let Some(book) = self.book.as_ref() else {
            return;
        };
        let Some(entity) = book.entity(entity_id).cloned() else {
            self.progression_status = Some("character not found".into());
            return;
        };

        let last = entity
            .progression
            .last()
            .map(|p| crate::llm::progression::LastSnapshot {
                chapter: p.chapter.clone(),
                voice_summary: p.voice_summary.clone(),
                notable_changes: p.notable_changes.clone(),
            });
        let user_prompt = crate::llm::progression::build_user(
            &entity.name,
            &entity.aliases,
            &entity.voice_notes,
            last.as_ref(),
            &ch.include_path,
            &prose,
        );
        let messages = vec![
            llm::ChatMessage::system(crate::llm::progression::SYSTEM_PROMPT.to_string()),
            llm::ChatMessage::user(user_prompt),
        ];
        // Output is a single small JSON snapshot per character; 1k is plenty.
        let tuning = llm::ChatTuning {
            temperature: 0.4,
            num_ctx: 32_768,
            num_predict: 1_024,
        };
        log::info!(
            "progression run start: entity={entity_id:?} chapter={:?} prose_chars={}",
            ch.include_path,
            prose.chars().count()
        );
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            true,
            tuning,
        );
        self.progression_stream = Some(handle);
        self.progression_target = Some((entity_id.to_string(), ch.include_path.clone()));
        self.progression_status = Some("extracting…".into());
    }

    pub fn poll_progression_stream(&mut self) {
        let Some(stream) = self.progression_stream.as_mut() else {
            return;
        };
        let _ = stream.poll();
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.progression_stream = None;
        let Some((entity_id, chapter)) = self.progression_target.take() else {
            return;
        };
        if let Some(e) = err {
            log::error!("progression stream error: {e}");
            self.progression_status = Some(format!("error: {e}"));
            return;
        }
        let Some(raw) = crate::llm::progression::parse(&buffer) else {
            log::warn!(
                "progression: could not parse JSON ({} bytes returned)",
                buffer.len()
            );
            self.progression_status =
                Some("could not parse JSON from model response (see log)".into());
            return;
        };
        if raw.is_empty() {
            log::info!(
                "progression: model returned empty snapshot for {entity_id:?} in {chapter:?} — character likely absent"
            );
            self.progression_status = Some("character not present in this chapter".into());
            return;
        }
        let Some(book) = self.book.as_mut() else {
            return;
        };
        let Some(mut entity) = book.entity(&entity_id).cloned() else {
            return;
        };
        entity
            .progression
            .push(crate::book::entity::ProgressionEntry {
                chapter: chapter.clone(),
                tone: raw.tone.trim().to_string(),
                situation: raw.situation.trim().to_string(),
                voice_summary: raw.voice_summary.trim().to_string(),
                notable_changes: raw.notable_changes.trim().to_string(),
            });
        if let Err(err) = book.save_entity(entity) {
            log::error!("progression save_entity failed: {err}");
            self.progression_status = Some(format!("save failed: {err}"));
            return;
        }
        log::info!("progression: appended snapshot for {entity_id:?} in {chapter:?}");
        self.progression_status = Some(format!("snapshot saved for {chapter}"));
    }
}
