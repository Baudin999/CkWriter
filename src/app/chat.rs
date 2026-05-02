use crate::book::latex;
use crate::llm;
use crate::scope;

impl super::CkWriterApp {
    /// Send the current chat input to the model. Builds a fresh system prompt
    /// with the live chapter prose so the assistant always answers against
    /// what's on screen, not a stale snapshot.
    pub fn send_chat_message(&mut self) {
        if self.chat_stream.is_some() {
            return;
        }
        let text = self.chat_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if !self.ollama_ok {
            self.chat_error = Some("ollama unreachable".into());
            return;
        }
        let Some(book) = self.book.as_ref() else {
            self.chat_error = Some("open a book first".into());
            return;
        };
        let Some(ch) = self.current_chapter.clone() else {
            self.chat_error = Some("open a chapter first".into());
            return;
        };
        let prose = latex::to_prose(&self.editor_text);
        if prose.trim().is_empty() {
            self.chat_error = Some("the chapter is empty".into());
            return;
        }
        let in_scope = scope::voice_context_entities(book, &self.entity_hits);
        let system =
            crate::llm::conversation::build_system(book, &in_scope, &ch.display_title, &prose);

        self.chat_messages.push(llm::ChatMessage::user(text));
        self.chat_input.clear();
        self.chat_error = None;
        self.chat_chapter = Some(ch.file_path.clone());

        let mut messages = Vec::with_capacity(self.chat_messages.len() + 1);
        messages.push(llm::ChatMessage::system(system));
        messages.extend(self.chat_messages.iter().cloned());

        let tuning = llm::ChatTuning {
            temperature: 0.6,
            num_ctx: 32_768,
            num_predict: 1_024,
        };
        log::info!(
            "chat send: chapter={:?} history_turns={} prose_chars={}",
            ch.display_title,
            self.chat_messages.len(),
            prose.chars().count(),
        );
        let handle = llm::chat_stream(
            &self.settings.ollama_url,
            &self.settings.model,
            messages,
            false,
            tuning,
        );
        self.chat_stream = Some(handle);
        self.chat_pending_assistant.clear();
    }

    pub fn poll_chat_stream(&mut self) {
        let Some(stream) = self.chat_stream.as_mut() else {
            return;
        };
        let _ = stream.poll();
        if !stream.buffer.is_empty() {
            self.chat_pending_assistant = stream.buffer.clone();
        }
        if !stream.done {
            return;
        }
        let buffer = std::mem::take(&mut stream.buffer);
        let err = stream.error.take();
        self.chat_stream = None;
        if let Some(e) = err {
            log::error!("chat stream error: {e}");
            self.chat_error = Some(e);
            self.chat_pending_assistant.clear();
            return;
        }
        let trimmed = buffer.trim();
        if trimmed.is_empty() {
            self.chat_error = Some("model returned an empty response".into());
            self.chat_pending_assistant.clear();
            return;
        }
        self.chat_messages
            .push(llm::ChatMessage::assistant(trimmed.to_string()));
        self.chat_pending_assistant.clear();
    }

    pub fn reset_chat(&mut self) {
        self.chat_messages.clear();
        self.chat_input.clear();
        self.chat_stream = None;
        self.chat_pending_assistant.clear();
        self.chat_error = None;
        self.chat_chapter = None;
    }
}
