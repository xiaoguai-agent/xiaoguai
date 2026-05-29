//! MiniMax provider backend (DEC-024 / Sprint-8 S8-10).
//!
//! Endpoint: `https://api.minimax.io/v1/chat/completions`
//! Auth: `Authorization: Bearer <key>` (OpenAI-compatible)
//!
//! MiniMax is OpenAI wire-compatible for messages + tool calls. The
//! distinguishing feature is **thinking-mode passthrough**: the M1/M2
//! family streams a `reasoning_content` field on each choice-delta
//! carrying the model's chain-of-thought. We surface it via
//! [`ChatChunk::reasoning_delta`] so the agent loop can record / display
//! / feed-to-a-Critic without conflating it with the assistant message
//! `content`.
//!
//! **Supported models** (pass verbatim as `ChatRequest::model`):
//!   - `MiniMax-M1`
//!   - `MiniMax-M2`
//!   - `MiniMax-M2.5`
//!   - `MiniMax-M2.7`
//!   - `abab6.5-chat` (no reasoning track; the field stays unset)

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

/// Default MiniMax HTTPS base URL.
pub const MINIMAX_DEFAULT_BASE: &str = "https://api.minimax.io";

#[derive(Debug, Clone)]
pub struct MinimaxBackend {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl MinimaxBackend {
    /// Production constructor. `api_key` is the `MINIMAX_API_KEY` secret.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(MINIMAX_DEFAULT_BASE, api_key)
    }

    /// Test constructor — allows pointing at a mockito server.
    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }
}

// ── Request shapes (OpenAI-wire compatible) ────────────────────────────────

#[derive(Serialize)]
struct MmRequest<'a> {
    model: &'a str,
    messages: Vec<MmMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<MmTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<JsonValue>,
}

#[derive(Serialize)]
struct MmMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<MmOutgoingToolCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct MmOutgoingToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: MmOutgoingFn<'a>,
}

#[derive(Serialize)]
struct MmOutgoingFn<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct MmTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: MmToolFn<'a>,
}

#[derive(Serialize)]
struct MmToolFn<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
}

// ── Response shapes ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MmSseChunk {
    #[serde(default)]
    choices: Vec<MmChoice>,
}

#[derive(Deserialize)]
struct MmChoice {
    #[serde(default)]
    delta: MmDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct MmDelta {
    #[serde(default)]
    content: Option<String>,
    /// MiniMax M1/M2 thinking-mode delta.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<MmIncomingToolCallDelta>,
}

#[derive(Deserialize)]
struct MmIncomingToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<MmIncomingFnDelta>,
}

#[derive(Deserialize)]
struct MmIncomingFnDelta {
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

fn build_messages(messages: &[Message]) -> Vec<MmMessage<'_>> {
    messages
        .iter()
        .map(|m| MmMessage {
            role: role_str(m.role),
            content: &m.content,
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| MmOutgoingToolCall {
                    id: &tc.id,
                    kind: "function",
                    function: MmOutgoingFn {
                        name: &tc.name,
                        arguments: &tc.arguments_json,
                    },
                })
                .collect(),
            tool_call_id: m.tool_call_id.as_deref(),
        })
        .collect()
}

fn build_tools(tools: &[ToolSpec]) -> Vec<MmTool<'_>> {
    tools
        .iter()
        .map(|t| MmTool {
            kind: "function",
            function: MmToolFn {
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
    fn merge(&mut self, delta: MmIncomingToolCallDelta) {
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
        let id = self.id.unwrap_or_else(|| format!("call_minimax_{index}"));
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
        reasoning_delta: None,
    })
}

/// Record reasoning bytes on the per-(provider,model) Prometheus counter.
/// Uses [`crate::token_count::estimate_tokens`] for the 4-char/token
/// heuristic the rest of the LLM crate relies on.
fn record_reasoning_tokens(model: &str, reasoning: &str) {
    let tokens = crate::token_count::estimate_tokens(reasoning);
    if tokens == 0 {
        return;
    }
    if let Some(counter) = xiaoguai_observability::prometheus::llm_reasoning_tokens_total() {
        counter
            .with_label_values(&["minimax", model])
            .inc_by(tokens as u64);
    }
}

#[async_trait]
impl LlmBackend for MinimaxBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let body = MmRequest {
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
        let model = req.model.clone();

        let mapped = {
            let partials = partials.clone();
            let final_emitted = final_emitted.clone();
            let model = model.clone();
            sse.map(move |ev| {
                let ev = ev.map_err(|e| LlmError::Network(e.to_string()))?;
                if ev.data == "[DONE]" {
                    return Ok(emit_final(&partials, &final_emitted, None));
                }
                let parsed: MmSseChunk = serde_json::from_str(&ev.data)
                    .map_err(|e| LlmError::Provider(format!("decode SSE: {e}")))?;
                let mut delta = String::new();
                let mut reasoning = String::new();
                let mut finish: Option<FinishReason> = None;
                for choice in parsed.choices {
                    if let Some(c) = choice.delta.content {
                        delta.push_str(&c);
                    }
                    if let Some(r) = choice.delta.reasoning_content {
                        reasoning.push_str(&r);
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
                if !reasoning.is_empty() {
                    record_reasoning_tokens(&model, &reasoning);
                }
                if let Some(reason) = finish {
                    Ok(emit_final(&partials, &final_emitted, Some(reason)))
                } else {
                    let reasoning_delta = (!reasoning.is_empty()).then_some(reasoning);
                    if delta.is_empty() && reasoning_delta.is_none() {
                        Ok(None)
                    } else {
                        Ok(Some(ChatChunk {
                            delta,
                            reasoning_delta,
                            ..Default::default()
                        }))
                    }
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
        "minimax"
    }
}
