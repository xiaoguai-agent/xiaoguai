//! `RuntimeSink` — caller-side hooks for streaming + finalisation.
//!
//! Three call sites in the wider workspace need different granularity:
//!
//! * REST wants each [`AgentEvent`] as an SSE chunk (via [`crate::run_streamed`]).
//! * IM wants only the final outcome (via [`crate::run_to_completion`]).
//! * Scheduler wants a callback per attempt with the final outcome (via
//!   [`crate::run_to_sink`] — it implements both hooks but ignores events
//!   and persists summary into a `JobRun` row).
//!
//! The default impls of both methods are no-ops so a caller writes only
//! the hook they actually need.

use async_trait::async_trait;
use xiaoguai_agent::AgentEvent;

use crate::error::RuntimeError;
use crate::outcome::RuntimeOutcome;

#[async_trait]
pub trait RuntimeSink: Send + Sync {
    /// Called for each event the agent emits. Returning an error does
    /// NOT abort the agent loop — the runtime captures the first sink
    /// error and surfaces it from `run_to_sink` once the loop finishes,
    /// so a sink can fail soft without aborting an in-flight run.
    async fn on_event(&self, _event: &AgentEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    /// Called once after the loop terminates with the full outcome.
    /// An error here is surfaced from `run_to_sink` — it's the last
    /// chance for the caller to fail.
    async fn on_finish(&self, _outcome: &RuntimeOutcome) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// No-op sink. Useful for `run_to_sink` callers that only want the
/// returned outcome (equivalent to `run_to_completion` then).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

#[async_trait]
impl RuntimeSink for NoopSink {}
