//! Shared agent-loop runtime.
//!
//! Before v0.12.0, three call sites in the workspace all built and ran a
//! [`xiaoguai_agent::ReactAgent`] with near-identical glue:
//!
//! * REST `send_message` (`xiaoguai-api`) — persist user msg → load history
//!   → run-stream → SSE → finalize task persists new msgs.
//! * IM `run_agent_and_reply` (`xiaoguai-im-gateway`) — snapshot history →
//!   append user → run-to-completion → pick reply text → persist full new
//!   slice.
//! * Scheduler `JobExecutor` (`xiaoguai-scheduler`) — until v0.12.0 only a
//!   stub; the agent-driven executor lands together with the runtime.
//!
//! Each call site duplicated 30–80 lines of "build agent, find reply text,
//! slice new messages". This crate extracts that into one place behind three
//! entry points:
//!
//! * [`run_to_completion`] — block, return [`RuntimeOutcome`]. IM uses this.
//! * [`run_streamed`] — return `(join_handle, event_stream)`. REST uses this.
//! * [`run_to_sink`] — drive a [`RuntimeSink`]'s `on_event` + `on_finish`.
//!   Scheduler uses this so the audit appender + JobRun-row update both
//!   hang off one hook instead of being open-coded.
//!
//! [`RuntimeOutcome`] enriches the underlying [`xiaoguai_agent::AgentOutcome`]
//! with two derived fields callers all wanted independently:
//! [`RuntimeOutcome::reply_text`] (last non-empty assistant message) and
//! [`RuntimeOutcome::new_messages`] (slice produced by this run).
//!
//! ## Dependency direction
//!
//! `xiaoguai-runtime` deliberately depends **only** on `xiaoguai-agent` +
//! `xiaoguai-llm`. It does NOT depend on `xiaoguai-storage`, `xiaoguai-api`,
//! or `xiaoguai-core` — the arrow points the other way. Callers wrap the
//! runtime, not the reverse. This keeps the runtime trivially test-stubbable
//! and prevents the v0.12.0 extraction from creating a new "everything
//! depends on everything" hub.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod error;
pub mod outcome;
pub mod resilience;
pub mod sink;

use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, ReactAgent, Toolbox};
use xiaoguai_llm::{LlmBackend, Message};

pub use error::RuntimeError;
pub use outcome::RuntimeOutcome;
pub use resilience::{
    BreakerConfig, BreakerOpened, BreakerState, CircuitBreaker, EscalationBus, ResilienceError,
    RetryPolicy, WallClock, BREAKER_LLM, BREAKER_PG, BREAKER_WEBHOOK,
};
pub use sink::{NoopSink, RuntimeSink};

/// Context shared across every runtime entry point: backend, toolbox,
/// and the agent configuration. Construct once, reuse across many calls.
#[derive(Clone)]
pub struct RuntimeContext {
    pub backend: Arc<dyn LlmBackend>,
    pub toolbox: Arc<Toolbox>,
    pub agent_config: AgentConfig,
}

impl RuntimeContext {
    #[must_use]
    pub fn new(
        backend: Arc<dyn LlmBackend>,
        toolbox: Arc<Toolbox>,
        agent_config: AgentConfig,
    ) -> Self {
        Self {
            backend,
            toolbox,
            agent_config,
        }
    }

    /// Return a clone with `agent_config.model` overridden. Used by the
    /// REST handler which lets a per-request `model` override the session
    /// default.
    #[must_use]
    pub fn with_model(&self, model: impl Into<String>) -> Self {
        let mut cfg = self.agent_config.clone();
        cfg.model = model.into();
        Self {
            backend: self.backend.clone(),
            toolbox: self.toolbox.clone(),
            agent_config: cfg,
        }
    }

    fn build_agent(&self) -> ReactAgent {
        ReactAgent::new(
            self.backend.clone(),
            (*self.toolbox).clone(),
            self.agent_config.clone(),
        )
    }

    /// Find the inbound prompt — the last user-role message in `history`.
    /// Returns empty string if there is no user message (caller passed a
    /// system-only or empty history). Used by all three entry points to
    /// compute [`RuntimeOutcome::new_messages`].
    fn inbound_prompt(history: &[Message]) -> String {
        history
            .iter()
            .rev()
            .find(|m| matches!(m.role, xiaoguai_llm::Role::User))
            .map_or_else(String::new, |m| m.content.clone())
    }
}

/// Run the agent loop to completion. Blocks until the loop terminates.
/// Returns the enriched outcome. Used by the IM gateway and by scheduler
/// callers that don't need streaming.
pub async fn run_to_completion(
    ctx: &RuntimeContext,
    history: Vec<Message>,
    cancel: CancellationToken,
) -> Result<RuntimeOutcome, RuntimeError> {
    let inbound = RuntimeContext::inbound_prompt(&history);
    let agent = ctx.build_agent();
    let (outcome, _events) = agent.run_to_completion(history, cancel).await?;
    Ok(RuntimeOutcome::from_agent(outcome, &inbound))
}

/// Run the agent loop with a streaming event channel. Returns
/// `(join_handle, event_stream)` — the stream completes when the loop
/// terminates; the join future yields the enriched outcome. Used by REST
/// `send_message` for SSE.
#[must_use]
pub fn run_streamed(
    ctx: &RuntimeContext,
    history: Vec<Message>,
    cancel: CancellationToken,
) -> (
    JoinHandle<Result<RuntimeOutcome, RuntimeError>>,
    ReceiverStream<xiaoguai_agent::AgentEvent>,
) {
    let inbound = RuntimeContext::inbound_prompt(&history);
    let agent = ctx.build_agent();
    let (join, stream) = agent.run_stream(history, cancel);
    let enriched = tokio::spawn(async move {
        let outcome = join
            .await
            .map_err(|e| RuntimeError::Join(e.to_string()))??;
        Ok(RuntimeOutcome::from_agent(outcome, &inbound))
    });
    (enriched, stream)
}

/// Run the agent loop and feed every event to `sink.on_event`; on
/// termination, call `sink.on_finish` with the enriched outcome.
///
/// Sink errors during `on_event` do NOT abort the loop — the runtime
/// captures the first one and surfaces it from this function once the
/// loop finishes. Sink errors during `on_finish` are returned directly.
///
/// Used by the scheduler's `RuntimeJobExecutor` (v0.12.0) and by the
/// v0.10.2 proactive-trigger path in a future refactor (today the
/// scheduler runner calls the executor directly, so this hook is an
/// additive entry point, not a replacement).
pub async fn run_to_sink<S: RuntimeSink>(
    ctx: &RuntimeContext,
    history: Vec<Message>,
    sink: S,
    cancel: CancellationToken,
) -> Result<RuntimeOutcome, RuntimeError> {
    let inbound = RuntimeContext::inbound_prompt(&history);
    let agent = ctx.build_agent();
    let (join, mut stream) = agent.run_stream(history, cancel);

    // Drain the event stream into the sink. ReactAgent already buffers
    // emits via its own mpsc — a slow sink will simply pull events at its
    // own pace until the agent loop blocks on its send side. That's the
    // desired back-pressure behaviour: a hung sink stops the agent rather
    // than running it dry.
    let mut sink_err: Option<RuntimeError> = None;
    while let Some(ev) = stream.next().await {
        if let Err(e) = sink.on_event(&ev).await {
            if sink_err.is_none() {
                sink_err = Some(e);
            }
            // Keep draining so the agent loop's send side doesn't block.
        }
    }

    let agent_outcome = join
        .await
        .map_err(|e| RuntimeError::Join(e.to_string()))??;
    let outcome = RuntimeOutcome::from_agent(agent_outcome, &inbound);
    sink.on_finish(&outcome).await?;
    if let Some(e) = sink_err {
        return Err(e);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use xiaoguai_agent::AgentEvent;
    use xiaoguai_llm::MockBackend;

    fn ctx_with_response(text: &str) -> RuntimeContext {
        let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response(text));
        RuntimeContext::new(
            backend,
            Arc::new(Toolbox::new()),
            AgentConfig::new("mock-model"),
        )
    }

    #[tokio::test]
    async fn run_to_completion_returns_reply_text() {
        let ctx = ctx_with_response("hello back");
        let history = vec![Message::user("ping")];
        let outcome = run_to_completion(&ctx, history, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(outcome.reply_text, "hello back");
        assert!(!outcome.new_messages.is_empty());
        assert_eq!(outcome.new_messages[0].content, "ping");
    }

    #[tokio::test]
    async fn run_streamed_emits_events_and_joins() {
        let ctx = ctx_with_response("streamed");
        let history = vec![Message::user("hi")];
        let (join, mut stream) = run_streamed(&ctx, history, CancellationToken::new());
        let mut got_events = 0;
        while let Some(_ev) = stream.next().await {
            got_events += 1;
        }
        let outcome = join.await.unwrap().unwrap();
        assert!(got_events > 0, "stream should have at least one event");
        assert_eq!(outcome.reply_text, "streamed");
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<String>>,
        finished: Mutex<Option<String>>,
        fail_event: bool,
        fail_finish: bool,
    }

    #[async_trait]
    impl RuntimeSink for RecordingSink {
        async fn on_event(&self, event: &AgentEvent) -> Result<(), RuntimeError> {
            self.events.lock().push(format!("{event:?}"));
            if self.fail_event {
                return Err(RuntimeError::Sink("event boom".into()));
            }
            Ok(())
        }
        async fn on_finish(&self, outcome: &RuntimeOutcome) -> Result<(), RuntimeError> {
            *self.finished.lock() = Some(outcome.reply_text.clone());
            if self.fail_finish {
                return Err(RuntimeError::Sink("finish boom".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_to_sink_calls_both_hooks() {
        let ctx = ctx_with_response("sink reply");
        let sink = Arc::new(RecordingSink::default());
        let outcome = run_to_sink(
            &ctx,
            vec![Message::user("hi")],
            SinkRef(sink.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.reply_text, "sink reply");
        assert!(!sink.events.lock().is_empty());
        assert_eq!(sink.finished.lock().as_deref(), Some("sink reply"));
    }

    #[tokio::test]
    async fn run_to_sink_surfaces_first_event_error_after_finish() {
        let ctx = ctx_with_response("ok");
        let sink = Arc::new(RecordingSink {
            fail_event: true,
            ..Default::default()
        });
        let err = run_to_sink(
            &ctx,
            vec![Message::user("hi")],
            SinkRef(sink.clone()),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RuntimeError::Sink(_)));
        // on_finish ran even though on_event failed (we don't abort the loop).
        assert!(sink.finished.lock().is_some());
    }

    #[tokio::test]
    async fn run_to_sink_surfaces_finish_error() {
        let ctx = ctx_with_response("ok");
        let sink = Arc::new(RecordingSink {
            fail_finish: true,
            ..Default::default()
        });
        let err = run_to_sink(
            &ctx,
            vec![Message::user("hi")],
            SinkRef(sink),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RuntimeError::Sink(_)));
    }

    // Helper to use Arc<RecordingSink> with the RuntimeSink trait that
    // takes ownership.
    struct SinkRef(Arc<RecordingSink>);
    #[async_trait]
    impl RuntimeSink for SinkRef {
        async fn on_event(&self, event: &AgentEvent) -> Result<(), RuntimeError> {
            self.0.on_event(event).await
        }
        async fn on_finish(&self, outcome: &RuntimeOutcome) -> Result<(), RuntimeError> {
            self.0.on_finish(outcome).await
        }
    }

    #[tokio::test]
    async fn noop_sink_is_a_valid_runtime_sink() {
        let ctx = ctx_with_response("ok");
        let outcome = run_to_sink(
            &ctx,
            vec![Message::user("hi")],
            NoopSink,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.reply_text, "ok");
    }

}
