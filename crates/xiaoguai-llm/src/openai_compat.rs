//! OpenAI-compatible backend.
//!
//! Speaks the `/v1/chat/completions` SSE protocol shared by `OpenAI`, vLLM,
//! `DeepSeek`, 通义 (Dashscope-compat mode), 智谱, `SGLang`/`LMDeploy`, and
//! most self-hosted gateways. The base URL must already include the API
//! version prefix (e.g. `https://api.deepseek.com/v1`).

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest, Role};

#[derive(Debug, Clone)]
pub struct OpenAiCompatBackend {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAiCompatBackend {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct OpenAiMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OpenAiSseChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[async_trait]
impl LlmBackend for OpenAiCompatBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = OpenAiRequest {
            model: &req.model,
            messages: req
                .messages
                .iter()
                .map(|m| OpenAiMessage {
                    role: role_str(m.role),
                    content: &m.content,
                })
                .collect(),
            stream: true,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
        };

        let mut builder = self.http.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!("status {status}: {body}")));
        }

        // EventStream over the response body. Each Event carries one `data: …` line.
        let sse = resp.bytes_stream().eventsource();

        // Emit one synthesised `done: true` chunk when the upstream stream ends
        // without an explicit `[DONE]`, so downstream code only needs to watch
        // `chunk.done`.
        let mapped = sse
            .map(|ev| {
                let ev = ev.map_err(|e| LlmError::Network(e.to_string()))?;
                if ev.data == "[DONE]" {
                    return Ok(Some(ChatChunk {
                        delta: String::new(),
                        done: true,
                    }));
                }
                let parsed: OpenAiSseChunk = serde_json::from_str(&ev.data)
                    .map_err(|e| LlmError::Provider(format!("decode SSE: {e}")))?;
                let mut delta = String::new();
                let mut finished = false;
                for choice in parsed.choices {
                    if let Some(c) = choice.delta.content {
                        delta.push_str(&c);
                    }
                    if choice.finish_reason.is_some() {
                        finished = true;
                    }
                }
                Ok(Some(ChatChunk {
                    delta,
                    done: finished,
                }))
            })
            .filter_map(|res: Result<Option<ChatChunk>, LlmError>| async move {
                match res {
                    Ok(Some(c)) => Some(Ok(c)),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                }
            });

        // Append a sentinel `done: true` in case the upstream stream closes without [DONE].
        let with_sentinel = mapped.chain(futures::stream::once(async {
            Ok(ChatChunk {
                delta: String::new(),
                done: true,
            })
        }));

        // De-duplicate: once we have already emitted a done=true chunk, stop.
        let dedup = with_sentinel.scan(false, |seen_done, chunk_res| {
            let already = *seen_done;
            let val = match &chunk_res {
                Ok(c) if c.done => {
                    *seen_done = true;
                    if already {
                        None
                    } else {
                        Some(chunk_res)
                    }
                }
                _ if already => None,
                _ => Some(chunk_res),
            };
            futures::future::ready(val)
        });

        Ok(Box::pin(dedup))
    }

    fn name(&self) -> &'static str {
        "openai_compat"
    }
}
