//! Ollama backend — speaks the native Ollama `/api/chat` protocol.
//!
//! Tool calling (v0.5.4.1): Ollama (>= 0.3) supports OpenAI-style `tools` in
//! the request and returns completed calls in `message.tool_calls` — unlike the
//! OpenAI SSE shape, Ollama sends each call whole (object `arguments`, no
//! streamed deltas), so we map a chunk's `tool_calls` straight to
//! [`ChatChunk::tool_calls`]. Ollama's native API has no `tool_choice`, so that
//! field is ignored here (the model decides).

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest, FinishReason, Message, Role, ToolCallSpec, ToolSpec};

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool<'a>>,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'static str,
    content: &'a str,
    /// Assistant turns only — the function calls the model previously emitted,
    /// echoed back so the model has its own tool-call context.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OllamaOutgoingToolCall<'a>>,
}

/// Ollama message `tool_calls` entry: `{ "function": { name, arguments } }`.
/// `arguments` is a JSON **object** (Ollama), not the JSON **string** OpenAI uses.
#[derive(Serialize)]
struct OllamaOutgoingToolCall<'a> {
    function: OllamaOutgoingFn<'a>,
}

#[derive(Serialize)]
struct OllamaOutgoingFn<'a> {
    name: &'a str,
    arguments: JsonValue,
}

/// A tool definition in the request: `{ "type": "function", "function": {...} }`.
#[derive(Serialize)]
struct OllamaTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OllamaToolFn<'a>,
}

#[derive(Serialize)]
struct OllamaToolFn<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
}

#[derive(Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaChunkMessage>,
    done: bool,
}

#[derive(Deserialize)]
struct OllamaChunkMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaIncomingToolCall>,
}

#[derive(Deserialize)]
struct OllamaIncomingToolCall {
    function: OllamaIncomingFn,
}

#[derive(Deserialize)]
struct OllamaIncomingFn {
    name: String,
    /// Ollama returns arguments as a JSON object; we re-serialise to the string
    /// form `ToolCallSpec` carries.
    #[serde(default)]
    arguments: JsonValue,
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
        // Ollama >= 0.3 has a first-class `tool` role for results. (Pairing is
        // positional / by-name; Ollama messages carry no tool_call_id.)
        Role::Tool => "tool",
    }
}

/// Build the outgoing Ollama messages, echoing assistant `tool_calls`
/// (parsing each call's stored JSON-string arguments back into an object).
fn build_messages(messages: &[Message]) -> Vec<OllamaMessage<'_>> {
    messages
        .iter()
        .map(|m| OllamaMessage {
            role: role_str(m.role),
            content: &m.content,
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| OllamaOutgoingToolCall {
                    function: OllamaOutgoingFn {
                        name: &tc.name,
                        arguments: serde_json::from_str(&tc.arguments_json)
                            .unwrap_or(JsonValue::Object(serde_json::Map::new())),
                    },
                })
                .collect(),
        })
        .collect()
}

fn build_tools(tools: &[ToolSpec]) -> Vec<OllamaTool<'_>> {
    tools
        .iter()
        .map(|t| OllamaTool {
            kind: "function",
            function: OllamaToolFn {
                name: &t.name,
                description: t.description.as_deref(),
                parameters: &t.parameters,
            },
        })
        .collect()
}

#[async_trait]
impl LlmBackend for OllamaBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let body = OllamaRequest {
            model: &req.model,
            messages: build_messages(&req.messages),
            stream: true,
            tools: build_tools(&req.tools),
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
                let (delta, tool_calls) = match parsed.message {
                    Some(m) => {
                        // Ollama sends each call whole; synthesise stable ids
                        // (Ollama omits them) and re-serialise object args to
                        // the JSON-string form ToolCallSpec carries.
                        let calls = m
                            .tool_calls
                            .into_iter()
                            .enumerate()
                            .map(|(i, tc)| ToolCallSpec {
                                id: format!("call_{i}"),
                                name: tc.function.name,
                                arguments_json: serde_json::to_string(&tc.function.arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            })
                            .collect::<Vec<_>>();
                        (m.content, calls)
                    }
                    None => (String::new(), Vec::new()),
                };
                let finish_reason = if !tool_calls.is_empty() {
                    Some(FinishReason::ToolCalls)
                } else if parsed.done {
                    Some(FinishReason::Stop)
                } else {
                    None
                };
                Ok(ChatChunk {
                    delta,
                    tool_calls,
                    finish_reason,
                    done: parsed.done,
                    reasoning_delta: None,
                })
            });

        Ok(Box::pin(lines_stream))
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}
