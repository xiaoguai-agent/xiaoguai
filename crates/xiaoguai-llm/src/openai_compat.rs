//! OpenAI-compatible backend.
//!
//! Speaks the `/v1/chat/completions` SSE protocol shared by `OpenAI`, vLLM,
//! `DeepSeek`, 通义 (Dashscope-compat mode), 智谱, `SGLang`/`LMDeploy`, and
//! most self-hosted gateways. The base URL must already include the API
//! version prefix (e.g. `https://api.deepseek.com/v1`).
//!
//! v0.5.4 adds OpenAI-style tool calls. Streamed tool-call deltas are
//! accumulated inside the SSE adapter and surfaced as a single
//! `ChatChunk { tool_calls, done: true, finish_reason: ToolCalls }` on the
//! finish event. Text deltas continue to stream as they arrive.

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<JsonValue>,
}

#[derive(Serialize)]
struct OpenAiMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OpenAiOutgoingToolCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct OpenAiOutgoingToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiOutgoingFn<'a>,
}

#[derive(Serialize)]
struct OpenAiOutgoingFn<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct OpenAiTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiToolFn<'a>,
}

#[derive(Serialize)]
struct OpenAiToolFn<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
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
    #[serde(default)]
    tool_calls: Vec<OpenAiIncomingToolCallDelta>,
}

#[derive(Deserialize)]
struct OpenAiIncomingToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiIncomingFnDelta>,
}

#[derive(Deserialize)]
struct OpenAiIncomingFnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn build_messages(messages: &[Message]) -> Vec<OpenAiMessage<'_>> {
    messages
        .iter()
        .map(|m| OpenAiMessage {
            role: role_str(m.role),
            content: &m.content,
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| OpenAiOutgoingToolCall {
                    id: &tc.id,
                    kind: "function",
                    function: OpenAiOutgoingFn {
                        name: &tc.name,
                        arguments: &tc.arguments_json,
                    },
                })
                .collect(),
            tool_call_id: m.tool_call_id.as_deref(),
        })
        .collect()
}

fn build_tools(tools: &[ToolSpec]) -> Vec<OpenAiTool<'_>> {
    tools
        .iter()
        .map(|t| OpenAiTool {
            kind: "function",
            function: OpenAiToolFn {
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

/// Partial tool-call assembled from streamed deltas. Closed out by
/// `into_complete` when the finish event arrives.
#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn merge(&mut self, delta: OpenAiIncomingToolCallDelta) {
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
        let id = self.id.unwrap_or_else(|| format!("call_streamed_{index}"));
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

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    }
}

#[async_trait]
impl LlmBackend for OpenAiCompatBackend {
    // SSE wiring + tool-call accumulation + done-sentinel dedup live together
    // here because they share the per-request `partials` map. Splitting them
    // forces the map through an `Arc<Mutex<...>>` boundary that doesn't
    // simplify the code.
    #[allow(clippy::too_many_lines)]
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = OpenAiRequest {
            model: &req.model,
            messages: build_messages(&req.messages),
            stream: true,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tools: build_tools(&req.tools),
            tool_choice: build_tool_choice(&req.tool_choice),
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

        let sse = resp.bytes_stream().eventsource();

        // BTreeMap keyed by `index` so emitted tool-call order is stable.
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
                let parsed: OpenAiSseChunk = serde_json::from_str(&ev.data)
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

        // Sentinel `done: true` in case the upstream closes without [DONE] or
        // finish_reason. Skipped if we already emitted a final chunk.
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

        // Stop the stream once a done=true chunk has been observed.
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

/// Drain partial tool calls (if any) into a single terminal `ChatChunk`.
/// Returns `None` if the terminal chunk was already emitted (idempotent).
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
        reasoning_delta: None,
    })
}
