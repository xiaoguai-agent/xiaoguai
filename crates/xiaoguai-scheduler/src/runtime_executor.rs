//! `RuntimeJobExecutor` — production [`JobExecutor`] that runs each
//! scheduled job through the shared `xiaoguai_runtime`.
//!
//! v0.10.x shipped `EchoExecutor` as the test stub; v0.12.0 lands the
//! real one. It reads `job.payload.prompt` (string), builds a
//! single-user-turn history, applies the job's `tenant_id` to the
//! runtime context, and calls `run_to_completion`. The resulting
//! `RuntimeOutcome::reply_text` becomes the `ExecutionOutcome::output_preview`
//! (truncated to a reasonable length so the JobRun row + push payloads
//! stay light).
//!
//! Out of scope for v0.12.0 (deferred to v0.12.1):
//!
//! * **Per-run synthetic session.** `scheduled_job_runs.session_id` is
//!   nullable; today we leave it `None`. v0.12.1 will create a session
//!   on the fly so the audit-first console (v0.11.1) can drill into the
//!   scheduler-driven transcript.
//! * **Per-attempt RuntimeSink hookup.** Today we use `run_to_completion`
//!   because the `JobExecutor` trait already encapsulates retries. If
//!   v0.12.1 wants streaming AgentEvents into a sink (e.g. for the
//!   audit-first console's live progress view), we'll switch to
//!   `run_to_sink` then.

use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_runtime::{run_to_completion, RuntimeContext};

use crate::executor::{ExecutionOutcome, JobExecutor};
use crate::job::ScheduledJob;

const OUTPUT_PREVIEW_MAX: usize = 500;

pub struct RuntimeJobExecutor {
    ctx: Arc<RuntimeContext>,
}

impl RuntimeJobExecutor {
    #[must_use]
    pub fn new(ctx: Arc<RuntimeContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl JobExecutor for RuntimeJobExecutor {
    async fn execute(&self, job: &ScheduledJob, _attempt: u32) -> Result<ExecutionOutcome, String> {
        let prompt = job
            .payload
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "scheduled job payload missing string field `prompt`".to_string())?;

        let ctx = self.ctx.with_tenant(job.tenant_id.clone());
        let history = vec![LlmMessage::user(prompt)];
        let outcome = run_to_completion(&ctx, history, tokio_util::sync::CancellationToken::new())
            .await
            .map_err(|e| format!("runtime: {e}"))?;

        Ok(ExecutionOutcome {
            output_preview: truncate_preview(&outcome.reply_text),
            session_id: None,
        })
    }
}

fn truncate_preview(s: &str) -> String {
    if s.chars().count() <= OUTPUT_PREVIEW_MAX {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(OUTPUT_PREVIEW_MAX).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xiaoguai_agent::{AgentConfig, Toolbox};
    use xiaoguai_llm::{LlmBackend, MockBackend};

    use crate::job::ScheduledJob;
    use crate::trigger::Trigger;

    fn make_executor(reply: &str) -> RuntimeJobExecutor {
        let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response(reply));
        let ctx = Arc::new(RuntimeContext::new(
            backend,
            Arc::new(Toolbox::new()),
            AgentConfig::new("mock-model"),
        ));
        RuntimeJobExecutor::new(ctx)
    }

    fn make_job(prompt: serde_json::Value) -> ScheduledJob {
        ScheduledJob::new(
            "j1",
            Some("tenant-x".into()),
            "j1",
            Trigger::interval(60).unwrap(),
            serde_json::json!({ "prompt": prompt }),
        )
    }

    #[tokio::test]
    async fn execute_returns_reply_text_as_preview() {
        let exec = make_executor("hello scheduled world");
        let job = make_job(serde_json::Value::String("ping".into()));
        let outcome = exec.execute(&job, 1).await.unwrap();
        assert_eq!(outcome.output_preview, "hello scheduled world");
        assert!(outcome.session_id.is_none());
    }

    #[tokio::test]
    async fn execute_errors_when_prompt_missing() {
        let exec = make_executor("never reached");
        let job = make_job(serde_json::Value::Null);
        let err = exec.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("prompt"));
    }

    #[test]
    fn truncate_preview_caps_long_strings() {
        let long = "x".repeat(600);
        let out = truncate_preview(&long);
        assert!(
            out.chars().count() <= OUTPUT_PREVIEW_MAX + 1,
            "{}",
            out.len()
        );
        assert!(out.ends_with('…'));
    }
}
