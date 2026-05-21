//! Agent-loop events surfaced to callers.
//!
//! The ReAct loop streams these as it executes so an upstream `xiaoguai-api`
//! handler can forward them over SSE/WebSocket without coupling to the loop's
//! internals.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Streamed model text. Multiple events compose to the full assistant turn.
    TextDelta { delta: String },

    /// Model decided to call a tool. Emitted per call, before dispatch.
    ToolCallStarted {
        id: String,
        name: String,
        arguments: JsonValue,
    },

    /// Tool dispatch completed (success or failure).
    ToolCallFinished {
        id: String,
        name: String,
        ok: bool,
        /// MCP-side error message when `ok == false`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// MCP-side text payload (concatenated from text blocks).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_text: Option<String>,
    },

    /// One think→act→observe cycle completed.
    IterationCompleted { iteration: u32 },

    /// Loop terminated. No further events follow.
    Done { stop_reason: StopReason },

    /// Unrecoverable error mid-loop. No further events follow.
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model emitted a `finish_reason = stop` without further tool calls.
    Completed,
    /// Hit `AgentConfig::max_iterations` before the model stopped.
    MaxIterations,
    /// Caller signalled the cancellation token.
    Cancelled,
}
