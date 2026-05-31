//! Shared fixtures for the six §7 integration tests in
//! `triangle_*.rs`. Sprint-9 S9-6.
//!
//! Pattern (cribbed from `triangle::critic_agent::tests`):
//! `CannedBackend` returns a scripted sequence of `chat_stream`
//! responses and records every `ChatRequest` it saw.
//!
//! Each test wires three `CannedBackend`s (one per agent) and asserts
//! on the `OrchEvent` stream.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use parking_lot::Mutex;
use xiaoguai_llm::backend::ChatStream;
use xiaoguai_llm::{ChatChunk, ChatRequest, FinishReason, LlmBackend, LlmError};

/// Scripted backend with response sequence + request capture.
///
/// Calls pop from the FRONT of `responses` (FIFO). When the queue is
/// drained, the LAST element repeats — matches the
/// `CapturingBackend::chat_stream` semantics used in the
/// `planner_agent` tests so test scripts don't need to over-supply.
pub struct CannedBackend {
    responses: Mutex<Vec<String>>,
    captured: Mutex<Vec<ChatRequest>>,
    name: &'static str,
}

impl CannedBackend {
    pub fn new(name: &'static str, responses: Vec<&str>) -> Arc<Self> {
        assert!(
            !responses.is_empty(),
            "CannedBackend needs at least one response"
        );
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().map(String::from).collect()),
            captured: Mutex::new(Vec::new()),
            name,
        })
    }

    /// Read-only view of every `ChatRequest` this backend handled, in
    /// the order they arrived. Used by `triangle_scratchpad_quarantine`
    /// to inspect the Critic's system prompt.
    pub fn captured(&self) -> Vec<ChatRequest> {
        self.captured.lock().clone()
    }

    pub fn call_count(&self) -> usize {
        self.captured.lock().len()
    }
}

#[async_trait]
impl LlmBackend for CannedBackend {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.captured.lock().push(req);
        let text = {
            let mut guard = self.responses.lock();
            if guard.len() > 1 {
                guard.remove(0)
            } else {
                guard[0].clone()
            }
        };
        let chunk = ChatChunk {
            delta: text,
            reasoning_delta: None,
            tool_calls: vec![],
            finish_reason: Some(FinishReason::Stop),
            done: true,
        };
        Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
    }
}

// =====================================================================
// JSON helpers — keep the inline test setups readable.
// =====================================================================

/// Build a Planner JSON payload with the given (description, rubric)
/// pairs. All tasks are independent (no `depends_on_index`).
pub fn make_planner_response(goal: &str, tasks: &[(&str, &str)]) -> String {
    let tasks_json: Vec<String> = tasks
        .iter()
        .map(|(desc, rubric)| {
            format!(
                r#"{{
                    "description": "{}",
                    "acceptance_criteria": {{
                        "rubric": "{}",
                        "required_citation_pattern": null,
                        "min_confidence": null
                    }},
                    "depends_on_index": null
                }}"#,
                escape(desc),
                escape(rubric),
            )
        })
        .collect();
    format!(
        r#"{{
            "goal": "{}",
            "tasks": [{}]
        }}"#,
        escape(goal),
        tasks_json.join(",")
    )
}

/// Build a Critic JSON payload — `kind` ∈ "approve", "`request_revision`",
/// "reject"; the second arg is the reason/feedback string.
pub fn make_critic_response(kind: &str, body: &str) -> String {
    let field = match kind {
        "request_revision" => "feedback",
        _ => "reason",
    };
    format!(r#"{{"kind":"{kind}","{field}":"{}"}}"#, escape(body))
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
