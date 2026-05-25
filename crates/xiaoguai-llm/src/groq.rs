//! Groq fast-inference backend.
//!
//! Endpoint: `https://api.groq.com/openai/v1/chat/completions`
//! Auth: `Authorization: Bearer <key>` (OpenAI-compatible)
//!
//! Groq is fully OpenAI wire-compatible including streaming SSE and tool
//! calling — no custom parsing required. The only difference vs the generic
//! `openai_compat` backend is the fixed base URL.
//!
//! **Supported models** (pass verbatim as `ChatRequest::model`):
//!   - `llama-3.3-70b-versatile`
//!   - `mixtral-8x7b-32768`
//!   - `llama3-8b-8192`
//!   - `gemma2-9b-it`

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{
    ChatChunk, ChatRequest, FinishReason, Message, Role, ToolCallSpec, ToolChoice, ToolSpec,
};

pub const GROQ_DEFAULT_BASE: &str = "https://api.groq.com/openai";
const GROQ_BASE: &str = GROQ_DEFAULT_BASE;

#[derive(Debug, Clone)]
pub struct GroqBackend {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl GroqBackend {
    /// Production constructor. `api_key` is the `GROQ_API_KEY` secret.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(GROQ_BASE, api_key)
    }

    /// Test constructor — allows overriding the base URL to point at a mock.
    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }
}

// ── Request shapes ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct GroqRequest<'a> {
    model: &'a str,
    messages: Vec<GroqMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GroqTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<JsonValue>,
}

#[derive(Serialize)]
struct GroqMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<GroqOutgoingToolCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct GroqOutgoingToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: GroqOutgoingFn<'a>,
}

#[derive(Serialize)]
struct GroqOutgoingFn<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct GroqTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: GroqToolFn<'a>,
}

#[derive(Serialize)]
struct GroqToolFn<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
}

// ── Response shapes ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GroqSseChunk {
    #[serde(default)]
    choices: Vec<GroqChoice>,
}

#[derive(Deserialize)]
struct GroqChoice {
    #[serde(default)]
    delta: GroqDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct GroqDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<GroqIncomingToolCallDelta>,
}

#[derive(Deserialize)]
struct GroqIncomingToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<GroqIncomingFnDelta>,
}

#[derive(Deserialize)]
struct GroqIncomingFnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn build_messages(messages: &[Message]) -> Vec<GroqMessage<'_>> {
    messages
        .iter()
        .map(|m| GroqMessage {
            role: role_str(m.role),
            content: &m.content,
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| GroqOutgoingToolCall {
                    id: &tc.id,
                    kind: "function",
                    function: GroqOutgoingFn {
                        name: &tc.name,
                        arguments: &tc.arguments_json,
                    },
                })
                .collect(),
            tool_call_id: m.tool_call_id.as_deref(),
        })
        .collect()
}

fn build_tools(tools: &[ToolSpec]) -> Vec<GroqTool<'_>> {
    tools
        .iter()
        .map(|t| GroqTool {
            kind: "function",
            function: GroqToolFn {
                name: &t.name,
                description: t.description.as_deref(),
                parameters: &t.parameters,
            },
        })
        .collect()
}

fn build_tool_choice(c: &ToolChoice) -> Option<JsonValue> {
    match c {
        ToolChoice::Auto => None,
        ToolChoice::None => Some(JsonValue::String("none".to_string())),
        ToolChoice::Required => Some(JsonValue::String("required".to_string())),
        ToolChoice::Function(name) => Some(serde_json::json!({
            "type": "function",
            "function": { "name": name }
        })),
    }
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    }
}

#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn merge(&mut self, delta: GroqIncomingToolCallDelta) {
        if let Some(id) = delta.id {
            if !id.is_empty() {
                self.id = Some(id);
            }
        }
        if let Some(f) = delta.function {
            if let Some(name) = f.name {
                if !name.is_empty() {
                    self.name = Some(name);
                }
            }
            if let Some(args) = f.arguments {
                self.arguments.push_str(&args);
            }
        }
    }

    fn into_complete(self, index: u32) -> Option<ToolCallSpec> {
        let id = self.id.unwrap_or_else(|| format!("call_groq_{index}"));
        let name = self.name?;
        Some(ToolCallSpec {
            id,
            name,
            arguments_json: if self.arguments.is_empty() {
                "{}".to_string()
            } else {
                self.arguments
            },
        })
    }
}

fn emit_final(
    partials: &Arc<Mutex<BTreeMap<u32, PartialToolCall>>>,
    final_emitted: &Arc<Mutex<bool>>,
    finish: Option<FinishReason>,
) -> Option<ChatChunk> {
    let mut flag = final_emitted.lock().expect("final_emitted poisoned");
    if *flag {
        return None;
    }
    *flag = true;
    let mut map = partials.lock().expect("partials poisoned");
    let mut tool_calls = Vec::with_capacity(map.len());
    for (index, partial) in std::mem::take(&mut *map) {
        if let Some(tc) = partial.into_complete(index) {
            tool_calls.push(tc);
        }
    }
    let reason = finish.unwrap_or(if tool_calls.is_empty() {
        FinishReason::Stop
    } else {
        FinishReason::ToolCalls
    });
    Some(ChatChunk {
        delta: String::new(),
        tool_calls,
        finish_reason: Some(reason),
        done: true,
    })
}

// ── LlmBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmBackend for GroqBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let body = GroqRequest {
            model: &req.model,
            messages: build_messages(&req.messages),
            stream: true,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tools: build_tools(&req.tools),
            tool_choice: build_tool_choice(&req.tool_choice),
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!("status {status}: {body_text}")));
        }

        let sse = resp.bytes_stream().eventsource();

        let partials: Arc<Mutex<BTreeMap<u32, PartialToolCall>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let final_emitted = Arc::new(Mutex::new(false));

        let mapped = {
            let partials = partials.clone();
            let final_emitted = final_emitted.clone();
            sse.map(move |ev| {
                let ev = ev.map_err(|e| LlmError::Network(e.to_string()))?;
                if ev.data == "[DONE]" {
                    return Ok(emit_final(&partials, &final_emitted, None));
                }
                let parsed: GroqSseChunk = serde_json::from_str(&ev.data)
                    .map_err(|e| LlmError::Provider(format!("decode SSE: {e}")))?;
                let mut delta = String::new();
                let mut finish: Option<FinishReason> = None;
                for choice in parsed.choices {
                    if let Some(c) = choice.delta.content {
                        delta.push_str(&c);
                    }
                    {
                        let mut map = partials.lock().expect("partials poisoned");
                        for tc in choice.delta.tool_calls {
                            let index = tc.index;
                            map.entry(index).or_default().merge(tc);
                        }
                    }
                    if let Some(reason) = choice.finish_reason {
                        finish = Some(parse_finish_reason(&reason));
                    }
                }
                if let Some(reason) = finish {
                    Ok(emit_final(&partials, &final_emitted, Some(reason)))
                } else {
                    Ok(Some(ChatChunk {
                        delta,
                        ..Default::default()
                    }))
                }
            })
        }
        .filter_map(|res: Result<Option<ChatChunk>, LlmError>| async move {
            match res {
                Ok(Some(c)) => Some(Ok(c)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            }
        });

        let with_sentinel = {
            let partials = partials.clone();
            let final_emitted = final_emitted.clone();
            mapped.chain(futures::stream::once(async move {
                Ok(
                    emit_final(&partials, &final_emitted, None).unwrap_or_else(|| ChatChunk {
                        done: true,
                        finish_reason: Some(FinishReason::Stop),
                        ..Default::default()
                    }),
                )
            }))
        };

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
        "groq"
    }
}
