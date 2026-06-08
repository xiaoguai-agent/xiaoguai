//! Built-in `loop_done` / `loop_pause` tools for /loop ticks (LLD-LOOP-001
//! §3 "End condition", L2).
//!
//! These two tools are registered **only on loop turns** (a turn whose
//! `TurnInput.loop_id` is `Some`). The agent calls one when the loop's goal
//! is met (`loop_done`) or to stop ticking without completing
//! (`loop_pause`). The tools have no access to `AppState` — like every
//! agent tool they run inside the ReAct loop with no server context — so
//! they record the agent's *intent* into a shared [`LoopToolSink`]. The
//! controller reads that intent after the tick's turn finishes and applies
//! the terminal/paused transition (see `loops::drive`).
//!
//! This keeps the seam entirely in `xiaoguai-api`: an in-process
//! [`McpClient`] backing two [`Toolbox`] entries, no change to
//! `xiaoguai-runtime` or `xiaoguai-agent`.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value as JsonValue;
use xiaoguai_agent::Toolbox;
use xiaoguai_mcp::{McpClient, McpResult, ServerInfo, ToolDescriptor, ToolResult};

pub const LOOP_DONE_TOOL: &str = "loop_done";
pub const LOOP_PAUSE_TOOL: &str = "loop_pause";

/// What the agent asked the loop to do this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopToolKind {
    /// Goal met — terminalise the loop as `done`.
    Done,
    /// Stop ticking but keep the loop row (resumable by an operator) —
    /// move it to `paused`.
    Pause,
}

/// The intent a loop tick's agent recorded by calling `loop_done` /
/// `loop_pause`. `Done` always wins over `Pause`; otherwise the first call
/// wins (see [`LoopToolClient::record`]).
#[derive(Debug, Clone)]
pub struct LoopIntent {
    pub kind: LoopToolKind,
    pub reason: String,
}

/// Shared cell the loop tools write and the controller reads after the
/// tick's turn completes. `None` = the agent called neither tool.
pub type LoopToolSink = Arc<Mutex<Option<LoopIntent>>>;

/// In-process [`McpClient`] backing `loop_done` / `loop_pause`. Records the
/// agent's intent into the shared sink and returns a short confirmation the
/// agent can use to wrap up its final message.
struct LoopToolClient {
    sink: LoopToolSink,
}

impl LoopToolClient {
    /// Record the agent's intent. `Done` always wins (the stronger, terminal
    /// verdict) regardless of call order; between two same-kind calls the
    /// first reason is kept. So `pause` then `done`, or `done` then `pause`,
    /// both end the loop `done`; a second `done` keeps the first reason.
    fn record(&self, kind: LoopToolKind, reason: String) -> &'static str {
        let mut guard = self.sink.lock();
        let overwrite = match guard.as_ref() {
            None => true,
            // Only a Done may upgrade a previously-recorded Pause.
            Some(existing) => existing.kind == LoopToolKind::Pause && kind == LoopToolKind::Done,
        };
        if overwrite {
            *guard = Some(LoopIntent { kind, reason });
        }
        match kind {
            LoopToolKind::Done => "Loop marked done — this is the final tick.",
            LoopToolKind::Pause => "Loop paused — ticking stops; an operator can cancel it.",
        }
    }
}

fn reason_arg(args: &JsonValue) -> String {
    args.get("reason")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

#[async_trait]
impl McpClient for LoopToolClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "xiaoguai-loop".into(),
            version: "1".into(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(loop_tool_descriptors())
    }

    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        let kind = match name {
            LOOP_DONE_TOOL => LoopToolKind::Done,
            LOOP_PAUSE_TOOL => LoopToolKind::Pause,
            other => {
                return Ok(ToolResult {
                    text: format!("unknown loop tool: {other}"),
                    blocks: vec![],
                    is_error: true,
                })
            }
        };
        let text = self.record(kind, reason_arg(&args)).to_string();
        Ok(ToolResult {
            text,
            blocks: vec![],
            is_error: false,
        })
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

fn loop_tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: LOOP_DONE_TOOL.into(),
            description: Some(
                "End this recurring loop because its goal has been met. Call this \
                 when the thing the loop was watching for has happened (e.g. the CI \
                 run finished, the rollout is healthy). `reason` is a short summary \
                 shown to the operator. After calling it, write your final summary \
                 message; no further ticks run."
                    .into(),
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Short summary of why the loop is done." }
                },
                "required": ["reason"]
            }),
        },
        ToolDescriptor {
            name: LOOP_PAUSE_TOOL.into(),
            description: Some(
                "Pause this recurring loop without marking it done — ticking stops \
                 but the loop is kept so an operator can resume it. Use when you \
                 cannot make progress right now (e.g. waiting on a human, a transient \
                 outage). `reason` is shown to the operator."
                    .into(),
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Short summary of why the loop is paused." }
                },
                "required": ["reason"]
            }),
        },
    ]
}

/// Build a per-turn toolbox for a loop tick: the base toolbox plus the two
/// loop tools, sharing one [`LoopToolSink`]. Returns the new toolbox and the
/// sink (which the controller reads after the turn completes).
///
/// These are loop control tools, so they take precedence over any
/// same-named server tool via [`Toolbox::insert_or_replace`] — a server
/// tool called `loop_done` must never shadow the built-in (which would
/// leave the loop unable to self-terminate).
#[must_use]
pub fn with_loop_tools(base: &Toolbox) -> (Toolbox, LoopToolSink) {
    let sink: LoopToolSink = Arc::new(Mutex::new(None));
    let client: Arc<dyn McpClient> = Arc::new(LoopToolClient { sink: sink.clone() });
    let mut tb = base.clone();
    for descriptor in loop_tool_descriptors() {
        tb.insert_or_replace(client.clone(), descriptor);
    }
    (tb, sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loop_done_records_intent_and_confirms() {
        let (tb, sink) = with_loop_tools(&Toolbox::new());
        assert!(tb.get(LOOP_DONE_TOOL).is_some());
        assert!(tb.get(LOOP_PAUSE_TOOL).is_some());

        let entry = tb.get(LOOP_DONE_TOOL).unwrap();
        let res = entry
            .client
            .call_tool(
                LOOP_DONE_TOOL,
                serde_json::json!({ "reason": "CI went green" }),
            )
            .await
            .unwrap();
        assert!(!res.is_error);

        let intent = sink.lock().clone().expect("intent recorded");
        assert_eq!(intent.kind, LoopToolKind::Done);
        assert_eq!(intent.reason, "CI went green");
    }

    #[tokio::test]
    async fn first_intent_wins() {
        let (tb, sink) = with_loop_tools(&Toolbox::new());
        let entry = tb.get(LOOP_DONE_TOOL).unwrap().clone();
        entry
            .client
            .call_tool(
                LOOP_DONE_TOOL,
                serde_json::json!({ "reason": "done first" }),
            )
            .await
            .unwrap();
        // A later pause must NOT override the recorded done.
        let pause = tb.get(LOOP_PAUSE_TOOL).unwrap();
        pause
            .client
            .call_tool(LOOP_PAUSE_TOOL, serde_json::json!({ "reason": "too late" }))
            .await
            .unwrap();
        let intent = sink.lock().clone().unwrap();
        assert_eq!(intent.kind, LoopToolKind::Done);
        assert_eq!(intent.reason, "done first");
    }

    #[tokio::test]
    async fn done_upgrades_a_prior_pause() {
        // Done always wins regardless of order — a pause then a done ends
        // the loop done (the stronger, terminal verdict).
        let (tb, sink) = with_loop_tools(&Toolbox::new());
        let pause = tb.get(LOOP_PAUSE_TOOL).unwrap().clone();
        pause
            .client
            .call_tool(LOOP_PAUSE_TOOL, serde_json::json!({ "reason": "blocked" }))
            .await
            .unwrap();
        let done = tb.get(LOOP_DONE_TOOL).unwrap();
        done.client
            .call_tool(
                LOOP_DONE_TOOL,
                serde_json::json!({ "reason": "actually done" }),
            )
            .await
            .unwrap();
        let intent = sink.lock().clone().unwrap();
        assert_eq!(intent.kind, LoopToolKind::Done);
        assert_eq!(intent.reason, "actually done");
    }

    #[tokio::test]
    async fn pause_records_pause() {
        let (tb, sink) = with_loop_tools(&Toolbox::new());
        let entry = tb.get(LOOP_PAUSE_TOOL).unwrap();
        entry
            .client
            .call_tool(
                LOOP_PAUSE_TOOL,
                serde_json::json!({ "reason": "waiting on human" }),
            )
            .await
            .unwrap();
        assert_eq!(sink.lock().clone().unwrap().kind, LoopToolKind::Pause);
    }

    #[tokio::test]
    async fn missing_reason_defaults_to_empty() {
        let (tb, sink) = with_loop_tools(&Toolbox::new());
        let entry = tb.get(LOOP_DONE_TOOL).unwrap();
        entry
            .client
            .call_tool(LOOP_DONE_TOOL, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(sink.lock().clone().unwrap().reason, "");
    }
}
