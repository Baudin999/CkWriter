pub mod characters;
pub mod conversation;
mod parse;
pub mod progression;
pub mod prompts;
pub mod revision;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self {
        Self { role: "system".into(), content: s.into() }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self { role: "user".into(), content: s.into() }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: s.into() }
    }
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    /// Disable reasoning on thinking-capable models. We want JSON, not chain-of-thought.
    think: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    options: OllamaOptions,
}

#[derive(Debug, Serialize, Default)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    total_duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
    /// Reasoning tokens from thinking-capable models. Captured for diagnostics
    /// only — we do not surface them to the editor.
    #[serde(default)]
    thinking: String,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    Done,
    Error(String),
}

pub struct StreamHandle {
    pub rx: mpsc::Receiver<StreamEvent>,
    pub buffer: String,
    pub done: bool,
    pub error: Option<String>,
    _join: JoinHandle<()>,
}

impl StreamHandle {
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.rx.try_recv() {
            changed = true;
            match ev {
                StreamEvent::Token(t) => self.buffer.push_str(&t),
                StreamEvent::Done => self.done = true,
                StreamEvent::Error(e) => {
                    self.error = Some(e);
                    self.done = true;
                }
            }
        }
        changed
    }
}

/// Per-call tuning for `chat_stream`. Defaults match the existing voice/show/prose
/// pipelines; raise `num_ctx` for jobs whose prompts can exceed ~6k tokens
/// (e.g. character extraction on a long chapter).
#[derive(Debug, Clone, Copy)]
pub struct ChatTuning {
    pub temperature: f32,
    pub num_ctx: u32,
    pub num_predict: u32,
}

impl Default for ChatTuning {
    fn default() -> Self {
        Self {
            temperature: 0.4,
            num_ctx: 8192,
            num_predict: 2048,
        }
    }
}

pub fn chat_stream(
    ollama_url: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    json_mode: bool,
    tuning: ChatTuning,
) -> StreamHandle {
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let url = format!("{}/api/chat", ollama_url.trim_end_matches('/'));
    let model_owned = model.to_string();

    let join = thread::spawn(move || {
        if let Err(e) = run_stream(&url, &model_owned, &messages, json_mode, tuning, &tx) {
            let msg = format!("{e:#}");
            log::error!("ollama chat_stream failed: {msg}");
            let _ = tx.send(StreamEvent::Error(msg));
            let _ = tx.send(StreamEvent::Done);
        }
    });

    StreamHandle {
        rx,
        buffer: String::new(),
        done: false,
        error: None,
        _join: join,
    }
}

fn run_stream(
    url: &str,
    model: &str,
    messages: &[ChatMessage],
    json_mode: bool,
    tuning: ChatTuning,
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let start = std::time::Instant::now();
    let prompt_bytes: usize = messages.iter().map(|m| m.content.len()).sum();
    log::info!(
        "ollama request -> {url} model={model} messages={} prompt_bytes={prompt_bytes} json_mode={json_mode} num_ctx={} num_predict={}",
        messages.len(),
        tuning.num_ctx,
        tuning.num_predict,
    );
    // Rough char→token ratio of ~3.5 for english. Warn before the server
    // silently truncates the prompt; truncation drops the system message and
    // produces nonsense output (e.g. a critique instead of JSON).
    let est_tokens = (prompt_bytes as f32 / 3.5) as u32;
    if est_tokens > tuning.num_ctx {
        log::warn!(
            "ollama prompt likely exceeds context: est_tokens≈{est_tokens} num_ctx={} -- response will be truncated",
            tuning.num_ctx
        );
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let body = OllamaChatRequest {
        model,
        messages,
        stream: true,
        think: false,
        format: if json_mode { Some("json") } else { None },
        options: OllamaOptions {
            temperature: Some(tuning.temperature),
            num_ctx: Some(tuning.num_ctx),
            num_predict: Some(tuning.num_predict),
        },
    };

    let resp = match client.post(url).json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            log::error!(
                "ollama send failed after {:?}: {e}",
                start.elapsed()
            );
            return Err(e.into());
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().unwrap_or_default();
        log::error!(
            "ollama HTTP {status} after {:?}: {txt}",
            start.elapsed()
        );
        anyhow::bail!("ollama HTTP {status}: {txt}");
    }
    log::debug!("ollama HTTP {status}, streaming...");

    let reader = BufReader::new(resp);
    let mut tokens = 0usize;
    let mut bytes_out = 0usize;
    let mut thinking_bytes = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log::error!(
                    "ollama stream read failed after {:?} (tokens={tokens} bytes={bytes_out}): {e}",
                    start.elapsed()
                );
                return Err(e.into());
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<OllamaChatChunk>(&line) {
            Ok(chunk) => {
                if let Some(err) = chunk.error {
                    log::error!("ollama returned error chunk after {:?}: {err}", start.elapsed());
                    let _ = tx.send(StreamEvent::Error(err));
                }
                if let Some(m) = chunk.message {
                    thinking_bytes += m.thinking.len();
                    if !m.content.is_empty() {
                        tokens += 1;
                        bytes_out += m.content.len();
                        let _ = tx.send(StreamEvent::Token(m.content));
                    }
                }
                if chunk.done {
                    log::info!(
                        "ollama done in {:?}: tokens={tokens} bytes={bytes_out} thinking_bytes={thinking_bytes} done_reason={:?} prompt_eval={:?} eval={:?} server_total_ns={:?}",
                        start.elapsed(),
                        chunk.done_reason,
                        chunk.prompt_eval_count,
                        chunk.eval_count,
                        chunk.total_duration,
                    );
                    if bytes_out == 0 {
                        if thinking_bytes > 0 {
                            log::warn!(
                                "ollama returned thinking_bytes={thinking_bytes} but zero content -- think=false should have suppressed this"
                            );
                        } else {
                            log::warn!(
                                "ollama returned empty response: model may have failed to produce JSON, or num_ctx={} was too small for prompt_bytes={prompt_bytes}",
                                tuning.num_ctx
                            );
                        }
                    }
                    let _ = tx.send(StreamEvent::Done);
                    return Ok(());
                }
            }
            Err(e) => {
                log::warn!("ollama parse failed: {e} on line: {line}");
            }
        }
    }
    log::warn!(
        "ollama stream ended without done marker after {:?} (tokens={tokens} bytes={bytes_out})",
        start.elapsed()
    );
    let _ = tx.send(StreamEvent::Done);
    Ok(())
}

pub fn ping(ollama_url: &str) -> Result<Vec<String>> {
    let url = format!("{}/api/tags", ollama_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let resp = client.get(&url).send()?.error_for_status()?;
    #[derive(Deserialize)]
    struct Tags {
        models: Vec<TagModel>,
    }
    #[derive(Deserialize)]
    struct TagModel {
        name: String,
    }
    let tags: Tags = resp.json()?;
    Ok(tags.models.into_iter().map(|m| m.name).collect())
}
