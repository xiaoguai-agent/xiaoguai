//! Anthropic Messages API backend.
//!
//! Speaks the `/v1/messages` SSE protocol. Auth via `x-api-key` header +
//! `anthropic-version: 2023-06-01`. Supports streaming and tool use.
//!
//! **Supported model IDs** (pass verbatim as `ChatRequest::model`):
//!   - `claude-sonnet-4-6`
//!   - `claude-opus-4-7`
//!   - `claude-haiku-4-5`
//!
//! **Note**: 通义/DeepSeek/智谱 already work via `OpenAiCompatBackend` with
//! their respective base URLs — they are NOT duplicated here.

use std::collections::BTreeMap;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{
    ChatChunk, ChatRequest, FinishReason, Message, Role, ToolCallSpec, ToolChoice, ToolSpec,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";

#[derive(Debug, Clone)]
pub struct AnthropicBackend {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl AnthropicBackend {
    /// `base_url` should be `https://api.anthropic.com` (without trailing
    /// slash). `api_key` is the `ANTHROPIC_API_KEY` secret.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }
}

// ── Request shapes ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice<'a>>,
}

/// Anthropic message. Content can be a plain string or an array of content
/// blocks (for tool results and multi-part assistant messages).
#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: AnthropicContent,
}

#[derive(Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: JsonValue,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct AnthropicTool<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    input_schema: &'a JsonValue,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicToolChoice<'a> {
    Any,
    Tool { name: &'a str },
}

// ── SSE response shapes ────────────────────────────────────────────────────

/// Top-level SSE event types from the Anthropic streaming protocol.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    MessageStart {
        // Fields present but not used in our stream path; retained for
        // correct deserialization so unknown fields don't cause errors.
        #[allow(dead_code)]
        #[serde(default)]
        message: AnthropicMessageStartBody,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        #[allow(dead_code)]
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDeltaBody,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorBody,
    },
}

#[derive(Default, Deserialize)]
struct AnthropicMessageStartBody {
    #[allow(dead_code)]
    #[serde(default)]
    id: String,
    #[allow(dead_code)]
    #[serde(default)]
    model: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlockStart {
    Text {
        #[allow(dead_code)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct AnthropicMessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicErrorBody {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "end_turn" => FinishReason::Stop,
        "tool_use" => FinishReason::ToolCalls,
        "max_tokens" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    }
}

/// Split a `ChatRequest` message list into an optional system prompt string
/// and the user/assistant/tool turns that form the Anthropic `messages` array.
///
/// Anthropic requires `system` to be a top-level field, not a message.
/// Multiple system messages are joined with newlines.
fn split_system(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system_parts: Vec<&str> = Vec::new();
    let mut turns: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if !msg.content.is_empty() {
                    system_parts.push(&msg.content);
                }
            }
            Role::User => turns.push(AnthropicMessage {
                role: "user",
                content: AnthropicContent::Text(msg.content.clone()),
            }),
            Role::Assistant => {
                if msg.tool_calls.is_empty() {
                    turns.push(AnthropicMessage {
                        role: "assistant",
                        content: AnthropicContent::Text(msg.content.clone()),
                    });
                } else {
                    let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                    if !msg.content.is_empty() {
                        blocks.push(AnthropicContentBlock::Text {
                            text: msg.content.clone(),
                        });
                    }
                    for tc in &msg.tool_calls {
                        let input: JsonValue =
                            serde_json::from_str(&tc.arguments_json).unwrap_or(JsonValue::Null);
                        blocks.push(AnthropicContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input,
                        });
                    }
                    turns.push(AnthropicMessage {
                        role: "assistant",
                        content: AnthropicContent::Blocks(blocks),
                    });
                }
            }
            Role::Tool => {
                let tool_use_id = msg
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                turns.push(AnthropicMessage {
                    role: "user",
                    content: AnthropicContent::Blocks(vec![AnthropicContentBlock::ToolResult {
                        tool_use_id,
                        content: msg.content.clone(),
                    }]),
                });
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n"))
    };
    (system, turns)
}

fn build_tools(tools: &[ToolSpec]) -> Vec<AnthropicTool<'_>> {
    tools
        .iter()
        .map(|t| AnthropicTool {
            name: &t.name,
            description: t.description.as_deref(),
            input_schema: &t.parameters,
        })
        .collect()
}

fn build_tool_choice<'a>(
    choice: &'a ToolChoice,
    tools: &[ToolSpec],
) -> Option<AnthropicToolChoice<'a>> {
    if tools.is_empty() {
        return None;
    }
    match choice {
        // Auto and None: let the model decide (or omit tool entirely).
        ToolChoice::Auto | ToolChoice::None => None,
        ToolChoice::Required => Some(AnthropicToolChoice::Any),
        ToolChoice::Function(name) => Some(AnthropicToolChoice::Tool { name }),
    }
}

/// In-progress tool-use block being assembled from streamed deltas.
struct PartialToolUse {
    id: String,
    name: String,
    input_json: String,
}

impl PartialToolUse {
    fn into_spec(self) -> ToolCallSpec {
        ToolCallSpec {
            id: self.id,
            name: self.name,
            arguments_json: if self.input_json.is_empty() {
                "{}".to_string()
            } else {
                self.input_json
            },
        }
    }
}

/// Drain all partials from the map and convert to `ToolCallSpec` list.
fn drain_partials(partials: &mut BTreeMap<u32, PartialToolUse>) -> Vec<ToolCallSpec> {
    let keys: Vec<u32> = partials.keys().copied().collect();
    keys.into_iter()
        .filter_map(|k| partials.remove(&k).map(PartialToolUse::into_spec))
        .collect()
}

/// Process a single Anthropic SSE event into an optional `ChatChunk`.
///
/// Mutates `partials`, `finish`, and `done_emitted` in place.
fn process_event(
    parsed: AnthropicEvent,
    partials: &mut BTreeMap<u32, PartialToolUse>,
    finish: &mut Option<FinishReason>,
    done_emitted: &mut bool,
) -> Result<Option<ChatChunk>, LlmError> {
    match parsed {
        AnthropicEvent::Ping | AnthropicEvent::MessageStart { .. } => Ok(None),

        AnthropicEvent::Error { error } => Err(LlmError::Provider(format!(
            "anthropic error: {}",
            error.message
        ))),

        AnthropicEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            if let AnthropicContentBlockStart::ToolUse { id, name } = content_block {
                partials.insert(
                    index,
                    PartialToolUse {
                        id,
                        name,
                        input_json: String::new(),
                    },
                );
            }
            Ok(None)
        }

        AnthropicEvent::ContentBlockDelta { index, delta } => match delta {
            AnthropicDelta::TextDelta { text } => Ok(Some(ChatChunk {
                delta: text,
                ..Default::default()
            })),
            AnthropicDelta::InputJsonDelta { partial_json } => {
                if let Some(p) = partials.get_mut(&index) {
                    p.input_json.push_str(&partial_json);
                }
                Ok(None)
            }
        },

        AnthropicEvent::ContentBlockStop { .. } | AnthropicEvent::MessageDelta { .. } => {
            if let AnthropicEvent::MessageDelta { delta } = parsed {
                if let Some(reason) = delta.stop_reason {
                    *finish = Some(parse_finish_reason(&reason));
                }
            }
            Ok(None)
        }

        AnthropicEvent::MessageStop => {
            if *done_emitted {
                return Ok(None);
            }
            *done_emitted = true;
            let tool_calls = drain_partials(partials);
            let reason = finish.take().unwrap_or(if tool_calls.is_empty() {
                FinishReason::Stop
            } else {
                FinishReason::ToolCalls
            });
            Ok(Some(ChatChunk {
                tool_calls,
                finish_reason: Some(reason),
                done: true,
                ..Default::default()
            }))
        }
    }
}

// ── LlmBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmBackend for AnthropicBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            ANTHROPIC_MESSAGES_PATH
        );

        let (system, messages) = split_system(&req.messages);
        let tools = build_tools(&req.tools);
        let tool_choice = build_tool_choice(&req.tool_choice, &req.tools);

        let body = AnthropicRequest {
            model: &req.model,
            messages,
            system: system.as_deref(),
            // Default to 4096 tokens; respect caller's override.
            max_tokens: req.max_tokens.unwrap_or(4096),
            stream: true,
            temperature: req.temperature,
            tools,
            tool_choice,
        };

        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
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

        // Accumulated state for the streaming closure.
        let mut partials: BTreeMap<u32, PartialToolUse> = BTreeMap::new();
        let mut finish: Option<FinishReason> = None;
        let mut done_emitted = false;

        let mapped = sse.map(move |ev_res| {
            let ev = ev_res.map_err(|e| LlmError::Network(e.to_string()))?;
            let parsed: AnthropicEvent = serde_json::from_str(&ev.data)
                .map_err(|e| LlmError::Provider(format!("decode SSE: {e}")))?;
            process_event(parsed, &mut partials, &mut finish, &mut done_emitted)
        });

        let filtered = mapped.filter_map(|res| async move {
            match res {
                Ok(Some(c)) => Some(Ok(c)),
                Ok(None) => None,
                Err(e) => Some(Err(e)),
            }
        });

        Ok(Box::pin(filtered))
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }
}
