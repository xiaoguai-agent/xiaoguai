//! Cross-backend LLM types.
//!
//! v0.5.4 extends the v0.1 baseline with OpenAI-style tool calls so the agent
//! loop can drive parallel `function`-flavoured dispatch. Backends translate
//! these to/from their native protocols.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    /// `Role::Tool` carries a tool-execution result, paired with a
    /// `tool_call_id` referencing an earlier assistant `tool_calls` entry.
    Tool,
}

/// A model-emitted function call. `arguments_json` is the raw JSON string the
/// model produced — we keep it as a string so streaming concatenation works
/// before parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallSpec {
    pub id: String,
    pub name: String,
    /// Raw JSON string; callers should `serde_json::from_str` to interpret.
    pub arguments_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    /// Assistant role only — function calls produced by the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    /// Tool role only — references the assistant `tool_calls` entry being answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Assistant message carrying only tool calls (no text content).
    #[must_use]
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCallSpec>) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Tool-result message answering an earlier `tool_call_id`.
    #[must_use]
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// JSON-schema description of a callable tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema object describing the tool's arguments.
    pub parameters: JsonValue,
}

/// How the model is allowed to use the supplied tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides.
    #[default]
    Auto,
    /// Disallow tool calls entirely.
    None,
    /// Force the model to call at least one tool.
    Required,
    /// Force a specific function.
    Function(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "is_default_tool_choice")]
    pub tool_choice: ToolChoice,
}

fn is_default_tool_choice(c: &ToolChoice) -> bool {
    matches!(c, ToolChoice::Auto)
}

impl ChatRequest {
    /// Minimal builder for the common case (no tools, no temperature override).
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        }
    }
}

/// Why the model stopped emitting tokens. `None` on intermediate chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Other(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub delta: String,
    /// Backends accumulate streamed tool-call deltas and emit the completed
    /// list on the final chunk (`done = true, finish_reason = Some(ToolCalls)`).
    /// Intermediate chunks leave this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(default)]
    pub done: bool,
    /// Sprint-8 S8-10 (DEC-024): thinking-mode passthrough.
    ///
    /// Models that expose a separate reasoning track (`MiniMax` M1/M2 series,
    /// future DeepSeek-R, Anthropic extended thinking) emit reasoning bytes
    /// on a sibling channel to `delta`. Surfaced as a separate field so the
    /// caller can render it differently, record it for audit, or feed it
    /// to a Critic (DEC-021). Backends without a reasoning channel leave
    /// this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_delta: Option<String>,
}
