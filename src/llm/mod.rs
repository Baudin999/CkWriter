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
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
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
}

#[derive(Debug, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
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

pub fn chat_stream(
    ollama_url: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    json_mode: bool,
) -> StreamHandle {
    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let url = format!("{}/api/chat", ollama_url.trim_end_matches('/'));
    let model_owned = model.to_string();

    let join = thread::spawn(move || {
        if let Err(e) = run_stream(&url, &model_owned, &messages, json_mode, &tx) {
            let _ = tx.send(StreamEvent::Error(format!("{e:#}")));
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
    tx: &mpsc::Sender<StreamEvent>,
) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let body = OllamaChatRequest {
        model,
        messages,
        stream: true,
        format: if json_mode { Some("json") } else { None },
        options: OllamaOptions {
            temperature: Some(0.4),
            num_ctx: Some(8192),
        },
    };

    let resp = client.post(url).json(&body).send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let txt = resp.text().unwrap_or_default();
        anyhow::bail!("ollama HTTP {status}: {txt}");
    }

    let reader = BufReader::new(resp);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<OllamaChatChunk>(&line) {
            Ok(chunk) => {
                if let Some(err) = chunk.error {
                    let _ = tx.send(StreamEvent::Error(err));
                }
                if let Some(m) = chunk.message {
                    if !m.content.is_empty() {
                        let _ = tx.send(StreamEvent::Token(m.content));
                    }
                }
                if chunk.done {
                    let _ = tx.send(StreamEvent::Done);
                    return Ok(());
                }
            }
            Err(e) => {
                log::warn!("ollama parse failed: {e} on line: {line}");
            }
        }
    }
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
