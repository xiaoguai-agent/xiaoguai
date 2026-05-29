//! Azure OpenAI backend.
//!
//! Speaks the same `chat/completions` SSE protocol as `openai_compat.rs`
//! but uses Azure-specific:
//!   - Base URL: `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions`
//!   - Auth header: `api-key: <key>` (not `Authorization: Bearer`)
//!   - Version query param: `?api-version=2024-10-21`
//!
//! All SSE parsing is reused from `openai_compat` via `parse_sse_chunk`
//! and `emit_final`. The only differences are URL construction and auth.
//!
//! **Supported deployments**: any deployment name the operator configures
//! in their Azure OpenAI resource. Common examples: `gpt-4o`, `gpt-4o-mini`.
//!
//! The `model` field in `ChatRequest` is sent in the JSON body but Azure
//! ignores it — the deployment name (encoded in the URL) determines the
//! model. We pass it anyway for logging transparency.

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

const AZURE_API_VERSION: &str = "2024-10-21";

/// Azure OpenAI backend. Wraps a single deployment.
///
/// Construct with [`AzureOpenAiBackend::new`] or
/// [`AzureOpenAiBackend::with_base_url`] (the latter is for tests).
#[derive(Debug, Clone)]
pub struct AzureOpenAiBackend {
    /// Full base URL including resource + deployment path, without trailing
    /// slash. Example:
    /// `https://my-resource.openai.azure.com/openai/deployments/gpt-4o`
    endpoint: String,
    api_key: String,
    http: reqwest::Client,
}

impl AzureOpenAiBackend {
    /// Canonical constructor.
    ///
    /// `resource_name` — Azure resource name (subdomain)
    /// `deployment_name` — deployment name in that resource
    /// `api_key` — Azure `api-key` header value
    pub fn new(
        resource_name: impl AsRef<str>,
        deployment_name: impl AsRef<str>,
        api_key: impl Into<String>,
    ) -> Self {
        let endpoint = format!(
            "https://{}.openai.azure.com/openai/deployments/{}",
            resource_name.as_ref(),
            deployment_name.as_ref()
        );
        Self {
            endpoint,
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Test constructor — pass a full mockito base URL + deployment path.
    pub fn with_endpoint(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }
}

// ── Request / response shapes (identical to openai_compat) ────────────────

#[derive(Serialize)]
struct AzureRequest<'a> {
    model: &'a str,
    messages: Vec<AzureMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AzureTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<JsonValue>,
}

#[derive(Serialize)]
struct AzureMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "str::is_empty")]
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<AzureOutgoingToolCall<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct AzureOutgoingToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: AzureOutgoingFn<'a>,
}

#[derive(Serialize)]
struct AzureOutgoingFn<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct AzureTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: AzureToolFn<'a>,
}

#[derive(Serialize)]
struct AzureToolFn<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
}

#[derive(Deserialize)]
struct AzureSseChunk {
    #[serde(default)]
    choices: Vec<AzureChoice>,
}

#[derive(Deserialize)]
struct AzureChoice {
    #[serde(default)]
    delta: AzureDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct AzureDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<AzureIncomingToolCallDelta>,
}

#[derive(Deserialize)]
struct AzureIncomingToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<AzureIncomingFnDelta>,
}

#[derive(Deserialize)]
struct AzureIncomingFnDelta {
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

fn build_messages(messages: &[Message]) -> Vec<AzureMessage<'_>> {
    messages
        .iter()
        .map(|m| AzureMessage {
            role: role_str(m.role),
            content: &m.content,
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| AzureOutgoingToolCall {
                    id: &tc.id,
                    kind: "function",
                    function: AzureOutgoingFn {
                        name: &tc.name,
                        arguments: &tc.arguments_json,
                    },
                })
                .collect(),
            tool_call_id: m.tool_call_id.as_deref(),
        })
        .collect()
}

fn build_tools(tools: &[ToolSpec]) -> Vec<AzureTool<'_>> {
    tools
        .iter()
        .map(|t| AzureTool {
            kind: "function",
            function: AzureToolFn {
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
    fn merge(&mut self, delta: AzureIncomingToolCallDelta) {
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
        let id = self.id.unwrap_or_else(|| format!("call_azure_{index}"));
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

// ── LlmBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmBackend for AzureOpenAiBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        // Azure URL: <endpoint>/chat/completions?api-version=<version>
        let url = format!(
            "{}/chat/completions?api-version={}",
            self.endpoint.trim_end_matches('/'),
            AZURE_API_VERSION
        );

        let body = AzureRequest {
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
            .header("api-key", &self.api_key)
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
                let parsed: AzureSseChunk = serde_json::from_str(&ev.data)
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
        "azure_openai"
    }
}
