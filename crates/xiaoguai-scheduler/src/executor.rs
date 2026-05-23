//! Job executor — what actually runs when a job fires.
//!
//! v0.10.0 keeps this a single async trait so the production wiring
//! (running an agent loop, ingesting RAG sources, doing whatever the
//! job payload describes) is decoupled from the scheduler core.
//! v0.12.0 will introduce `xiaoguai_runtime::run_agent_to_sink` and
//! ship the canonical executor that calls into it.

use async_trait::async_trait;

use crate::job::ScheduledJob;

/// What an executor produces on success.
#[derive(Debug, Clone)]
pub struct ExecutionOutcome {
    /// Short preview of the final output. Surfaced in the `JobRun` row
    /// and in push payloads.
    pub output_preview: String,
    /// Optional session id created during execution (set when the
    /// executor produced a chat-style transcript).
    pub session_id: Option<String>,
}

/// Executor abstraction. Implementations should be cheap to clone or
/// share via `Arc`; the runner holds it as `Arc<dyn JobExecutor>`.
#[async_trait]
pub trait JobExecutor: Send + Sync {
    async fn execute(&self, job: &ScheduledJob, attempt: u32) -> Result<ExecutionOutcome, String>;
}

/// Trivial executor for tests + smoke. Echoes back the payload's
/// `prompt` field as the output preview.
#[derive(Debug, Default, Clone)]
pub struct EchoExecutor;

#[async_trait]
impl JobExecutor for EchoExecutor {
    async fn execute(&self, job: &ScheduledJob, _attempt: u32) -> Result<ExecutionOutcome, String> {
        let prompt = job
            .payload
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<no prompt>");
        Ok(ExecutionOutcome {
            output_preview: format!("echo: {prompt}"),
            session_id: None,
        })
    }
}
