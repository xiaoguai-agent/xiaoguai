//! Ollama backend — speaks the native Ollama `/api/chat` protocol.

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest, Role};

#[derive(Debug, Clone)]
pub struct OllamaBackend {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaChunkMessage>,
    done: bool,
}

#[derive(Deserialize)]
struct OllamaChunkMessage {
    content: String,
}

impl OllamaBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[async_trait]
impl LlmBackend for OllamaBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let body = OllamaRequest {
            model: &req.model,
            messages: req
                .messages
                .iter()
                .map(|m| OllamaMessage {
                    role: role_str(m.role),
                    content: &m.content,
                })
                .collect(),
            stream: true,
        };

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(LlmError::Provider(format!("status {}", resp.status())));
        }

        let bytes_stream = resp.bytes_stream();
        let lines_stream = bytes_stream
            .map(|chunk_res| chunk_res.map_err(|e| LlmError::Network(e.to_string())))
            .flat_map(|res| {
                stream::iter(match res {
                    Ok(bytes) => String::from_utf8_lossy(&bytes)
                        .split('\n')
                        .filter(|l| !l.is_empty())
                        .map(|s| Ok(s.to_string()))
                        .collect::<Vec<_>>(),
                    Err(e) => vec![Err(e)],
                })
            })
            .map(|line_res| {
                let line = line_res?;
                let parsed: OllamaChunk = serde_json::from_str(&line)
                    .map_err(|e| LlmError::Provider(format!("decode: {e}")))?;
                let delta = parsed.message.map(|m| m.content).unwrap_or_default();
                Ok(ChatChunk {
                    delta,
                    done: parsed.done,
                })
            });

        Ok(Box::pin(lines_stream))
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}
