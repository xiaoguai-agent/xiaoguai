//! Deterministic in-memory backend for tests and CI.
//!
//! v0.5.4 adds scripted multi-turn responses so the ReAct agent loop can be
//! exercised in unit tests without an external model. `with_script(...)`
//! returns one pre-baked response per `chat_stream` call, in order; the final
//! script step is replayed if the agent makes more calls than scripted.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest, FinishReason, ToolCallSpec};

/// One scripted response. `with_response(text)` is sugar for `text+stop`.
#[derive(Debug, Clone)]
pub struct ScriptStep {
    pub delta: String,
    pub tool_calls: Vec<ToolCallSpec>,
    pub finish_reason: FinishReason,
}

impl ScriptStep {
    #[must_use]
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            delta: content.into(),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
        }
    }

    #[must_use]
    pub fn tool_calls(calls: Vec<ToolCallSpec>) -> Self {
        Self {
            delta: String::new(),
            tool_calls: calls,
            finish_reason: FinishReason::ToolCalls,
        }
    }
}

#[derive(Debug, Clone)]
enum MockMode {
    Script(Arc<Mutex<Vec<ScriptStep>>>),
    Failing(LlmError),
}

#[derive(Debug, Clone)]
pub struct MockBackend {
    mode: MockMode,
}

impl MockBackend {
    pub fn with_response(response: impl Into<String>) -> Self {
        Self::with_script(vec![ScriptStep::text(response)])
    }

    /// Scripted multi-turn response — one step per `chat_stream` call.
    /// Once the script is exhausted the final step replays so callers don't
    /// need to know the exact iteration count.
    #[must_use]
    pub fn with_script(steps: Vec<ScriptStep>) -> Self {
        assert!(!steps.is_empty(), "with_script needs at least one step");
        Self {
            mode: MockMode::Script(Arc::new(Mutex::new(steps))),
        }
    }

    /// Backend that fails its initial `chat_stream` call with the given error.
    /// Used to exercise the router's fallback chain.
    #[must_use]
    pub fn failing(err: LlmError) -> Self {
        Self {
            mode: MockMode::Failing(err),
        }
    }
}

fn step_to_chunks(step: &ScriptStep) -> Vec<Result<ChatChunk, LlmError>> {
    let mut out = Vec::new();
    if !step.delta.is_empty() {
        out.push(Ok(ChatChunk {
            delta: step.delta.clone(),
            ..Default::default()
        }));
    }
    out.push(Ok(ChatChunk {
        delta: String::new(),
        tool_calls: step.tool_calls.clone(),
        finish_reason: Some(step.finish_reason.clone()),
        done: true,
    }));
    out
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        match &self.mode {
            MockMode::Failing(err) => Err(err.clone()),
            MockMode::Script(steps) => {
                let mut guard = steps.lock().expect("mock script lock poisoned");
                // Replay the last step indefinitely once exhausted.
                let step = if guard.len() > 1 {
                    guard.remove(0)
                } else {
                    guard[0].clone()
                };
                drop(guard);
                Ok(Box::pin(stream::iter(step_to_chunks(&step))))
            }
        }
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}
