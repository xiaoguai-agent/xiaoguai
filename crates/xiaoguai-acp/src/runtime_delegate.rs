//! An [`AcpDelegate`] backed by the shared agent runtime.
//!
//! This is the CLI path: a prompt turn appends the user text to the session's
//! in-memory history, drives [`xiaoguai_runtime::run_streamed`], maps each
//! [`AgentEvent`](xiaoguai_agent::AgentEvent) to a `session/update`, and stores
//! the resulting transcript back so the next turn continues the conversation.
//!
//! Per `LLD-ACP-001` §6 the loop runs under the owner's implicit authority
//! (the caller supplies a `RuntimeContext` whose gate is allow-all on the CLI
//! path), so turns never suspend; deny/escalate, if configured, surfaces as
//! text. Wiring the `xiaoguai-coding` tool surface into the loop's toolbox so an
//! IDE turn can edit code (not just chat) is the deferred coding-tool-
//! registration item.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use xiaoguai_llm::Message;
use xiaoguai_runtime::{run_streamed, RuntimeContext};

use crate::acp::StopReason;
use crate::delegate::{AcpDelegate, UpdateSink};
use crate::mapping;

/// Drives ACP prompt turns through the shared runtime, keeping per-session
/// history in memory for the life of the process.
pub struct RuntimeDelegate {
    ctx: RuntimeContext,
    history: Arc<Mutex<HashMap<String, Vec<Message>>>>,
}

impl RuntimeDelegate {
    /// Build a delegate over an already-wired runtime context.
    #[must_use]
    pub fn new(ctx: RuntimeContext) -> Self {
        Self {
            ctx,
            history: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl AcpDelegate for RuntimeDelegate {
    async fn prompt(
        &self,
        session_id: &str,
        prompt_text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> StopReason {
        // Snapshot the session's history and append the new user turn.
        let mut messages = {
            let store = self.history.lock().await;
            store.get(session_id).cloned().unwrap_or_default()
        };
        messages.push(Message::user(prompt_text));

        // L3 attribution: stamp the ACP session id for this turn so the
        // `token_usage` rows it produces are attributed (the router reads it off
        // each `ChatRequest`). NB the ACP id is `acp-<n>` from an in-process
        // counter that resets on restart, so it is process-scoped, not globally
        // stable — fine as an opaque usage key. Single-owner ⇒ no distinct user id.
        let ctx = self
            .ctx
            .with_attribution(Some(session_id.to_string()), None);
        let (join, mut events) = run_streamed(&ctx, messages, cancel);

        while let Some(ev) = events.next().await {
            if let Some(update) = mapping::map_event(&ev) {
                sink.send(update);
            }
        }

        match join.await {
            Ok(Ok(outcome)) => {
                // Persist the full transcript so the next turn continues.
                self.history
                    .lock()
                    .await
                    .insert(session_id.to_string(), outcome.messages.clone());
                mapping::map_stop_reason(&outcome.stop_reason)
            }
            Ok(Err(e)) => {
                sink.send(error_chunk(&format!("agent run failed: {e}")));
                StopReason::EndTurn
            }
            Err(e) => {
                sink.send(error_chunk(&format!("agent task panicked: {e}")));
                StopReason::EndTurn
            }
        }
    }
}

/// A `session/update` carrying a warning chunk for a turn-level failure.
fn error_chunk(message: &str) -> crate::acp::SessionUpdate {
    crate::acp::SessionUpdate::AgentMessageChunk(crate::acp::ContentChunk::new(
        crate::acp::ContentBlock::from(format!("⚠ {message}")),
    ))
}
