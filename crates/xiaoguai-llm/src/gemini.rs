//! Google Gemini `generateContent` backend.
//!
//! Speaks the REST `generateContent` (non-stream) and `streamGenerateContent`
//! (SSE) APIs. Auth via `key=<API_KEY>` query parameter.
//!
//! **Supported model IDs** (pass verbatim as `ChatRequest::model`):
//!   - `gemini-2.0-flash`
//!   - `gemini-2.5-pro`
//!
//! **Note**: 通义/DeepSeek/智谱 already work via `OpenAiCompatBackend` with
//! their respective base URLs — do not duplicate those here.

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{
    ChatChunk, ChatRequest, FinishReason, Message, Role, ToolCallSpec, ToolChoice, ToolSpec,
};

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com";

#[derive(Debug, Clone)]
pub struct GeminiBackend {
    /// Base URL (without trailing slash). Defaults to
    /// `https://generativelanguage.googleapis.com`.
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl GeminiBackend {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(GEMINI_BASE, api_key)
    }

    /// Allows overriding the base URL — used by tests to point at a mock
    /// server.
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
#[serde(rename_all = "camelCase")]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<GeminiToolConfig<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionCall {
    name: String,
    args: JsonValue,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionResponse {
    name: String,
    response: JsonValue,
}

#[derive(Serialize)]
struct GeminiSystemInstruction<'a> {
    parts: [GeminiTextPart<'a>; 1],
}

#[derive(Serialize)]
struct GeminiTextPart<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct GeminiTool<'a> {
    function_declarations: Vec<GeminiFunctionDecl<'a>>,
}

#[derive(Serialize)]
struct GeminiFunctionDecl<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    parameters: &'a JsonValue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolConfig<'a> {
    function_calling_config: GeminiFcConfig<'a>,
}

#[derive(Serialize)]
struct GeminiFcConfig<'a> {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_function_names: Option<Vec<&'a str>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

// ── Response shapes ────────────────────────────────────────────────────────

/// Streaming response line (each SSE data payload).
#[derive(Deserialize)]
struct GeminiStreamResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    }
}

/// Build the `contents` array and an optional `system_instruction` from
/// the flat `ChatRequest::messages` list.
///
/// Gemini requires the `system_instruction` as a separate top-level field.
/// Multiple system messages are joined with newlines.
fn build_contents(messages: &[Message]) -> (Vec<GeminiContent>, Option<String>) {
    let mut system_parts: Vec<&str> = Vec::new();
    let mut contents: Vec<GeminiContent> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => {
                if !msg.content.is_empty() {
                    system_parts.push(&msg.content);
                }
            }
            Role::User => {
                contents.push(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![GeminiPart::Text {
                        text: msg.content.clone(),
                    }],
                });
            }
            Role::Assistant => {
                let mut parts: Vec<GeminiPart> = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(GeminiPart::Text {
                        text: msg.content.clone(),
                    });
                }
                for tc in &msg.tool_calls {
                    let args: JsonValue =
                        serde_json::from_str(&tc.arguments_json).unwrap_or(JsonValue::Null);
                    parts.push(GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: tc.name.clone(),
                            args,
                        },
                    });
                }
                if !parts.is_empty() {
                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts,
                    });
                }
            }
            Role::Tool => {
                // Tool results come back as `function_response` parts in a
                // `user`-role content block.
                let name = msg
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                contents.push(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name,
                            response: serde_json::json!({ "result": msg.content }),
                        },
                    }],
                });
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n"))
    };
    (contents, system)
}

fn build_tools(tools: &[ToolSpec]) -> Vec<GeminiTool<'_>> {
    if tools.is_empty() {
        return Vec::new();
    }
    vec![GeminiTool {
        function_declarations: tools
            .iter()
            .map(|t| GeminiFunctionDecl {
                name: &t.name,
                description: t.description.as_deref(),
                parameters: &t.parameters,
            })
            .collect(),
    }]
}

fn build_tool_config<'a>(
    choice: &'a ToolChoice,
    tools: &[ToolSpec],
) -> Option<GeminiToolConfig<'a>> {
    if tools.is_empty() {
        return None;
    }
    match choice {
        ToolChoice::Auto => None,
        ToolChoice::None => Some(GeminiToolConfig {
            function_calling_config: GeminiFcConfig {
                mode: "NONE",
                allowed_function_names: None,
            },
        }),
        ToolChoice::Required => Some(GeminiToolConfig {
            function_calling_config: GeminiFcConfig {
                mode: "ANY",
                allowed_function_names: None,
            },
        }),
        ToolChoice::Function(name) => Some(GeminiToolConfig {
            function_calling_config: GeminiFcConfig {
                mode: "ANY",
                allowed_function_names: Some(vec![name.as_str()]),
            },
        }),
    }
}

/// Collect all text deltas and function calls from a candidate's content.
fn extract_candidate(
    candidate: &GeminiCandidate,
) -> (String, Vec<ToolCallSpec>, Option<FinishReason>) {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCallSpec> = Vec::new();

    if let Some(content) = &candidate.content {
        for part in &content.parts {
            match part {
                GeminiPart::Text { text: t } => text.push_str(t),
                GeminiPart::FunctionCall { function_call: fc } => {
                    tool_calls.push(ToolCallSpec {
                        id: format!("call_{}", fc.name),
                        name: fc.name.clone(),
                        arguments_json: fc.args.to_string(),
                    });
                }
                GeminiPart::FunctionResponse { .. } => {} // shouldn't appear in model output
            }
        }
    }

    let finish = candidate.finish_reason.as_deref().map(parse_finish_reason);
    (text, tool_calls, finish)
}

// ── LlmBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmBackend for GeminiBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        // Gemini streaming: POST `streamGenerateContent?alt=sse&key=<KEY>`
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url.trim_end_matches('/'),
            req.model,
            self.api_key
        );

        let (contents, system_text) = build_contents(&req.messages);
        let system_instruction = system_text.as_deref().map(|s| GeminiSystemInstruction {
            parts: [GeminiTextPart { text: s }],
        });
        let tools = build_tools(&req.tools);
        let tool_config = build_tool_config(&req.tool_choice, &req.tools);

        let gen_config = if req.temperature.is_some() || req.max_tokens.is_some() {
            Some(GeminiGenerationConfig {
                temperature: req.temperature,
                max_output_tokens: req.max_tokens,
            })
        } else {
            None
        };

        let body = GeminiRequest {
            contents,
            system_instruction,
            tools,
            tool_config,
            generation_config: gen_config,
        };

        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!("status {status}: {body}")));
        }

        // Gemini `alt=sse` streaming uses standard SSE format (`data:` lines).
        // Each `data:` payload is a JSON object (GeminiStreamResponse).
        let sse = resp.bytes_stream().eventsource();

        let mapped = sse.map(|ev_res| {
            let ev = ev_res.map_err(|e| LlmError::Network(e.to_string()))?;

            let parsed: GeminiStreamResponse = serde_json::from_str(&ev.data)
                .map_err(|e| LlmError::Provider(format!("decode SSE: {e} — raw: {}", ev.data)))?;

            let mut full_text = String::new();
            let mut tool_calls: Vec<ToolCallSpec> = Vec::new();
            let mut finish: Option<FinishReason> = None;

            for candidate in &parsed.candidates {
                let (t, tc, f) = extract_candidate(candidate);
                full_text.push_str(&t);
                tool_calls.extend(tc);
                if f.is_some() {
                    finish = f;
                }
            }

            let done = finish.is_some();
            let chunk = ChatChunk {
                delta: full_text,
                tool_calls,
                finish_reason: finish,
                done,
            };
            Ok(chunk)
        });

        // Ensure the stream ends with done=true even if the SSE closes without
        // a finish_reason.
        let with_sentinel = mapped.chain(futures::stream::once(async {
            Ok(ChatChunk {
                done: true,
                finish_reason: Some(FinishReason::Stop),
                ..Default::default()
            })
        }));

        // Deduplicate: stop after the first done=true.
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
        "gemini"
    }
}
