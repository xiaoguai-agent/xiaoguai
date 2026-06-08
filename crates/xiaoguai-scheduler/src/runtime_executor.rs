//! `RuntimeJobExecutor` — production [`JobExecutor`] that runs each
//! scheduled job through the shared `xiaoguai_runtime`.
//!
//! v0.10.x shipped `EchoExecutor` as the test stub; v0.12.0 lands the
//! real one. It reads `job.payload.prompt` (string), builds a
//! single-user-turn history, and calls `run_to_completion`. The resulting
//! `RuntimeOutcome::reply_text` becomes the `ExecutionOutcome::output_preview`
//! (truncated to a reasonable length so the `JobRun` row + push payloads
//! stay light).
//!
//! v0.12.1 adds the [`ScheduledSessionWriter`] hook. When the executor
//! is built with `Some(writer)` it creates a synthetic session per run
//! (so `scheduled_job_runs.session_id` is populated) and writes the
//! resulting `new_messages` slice into the `messages` table — the
//! v0.11.1 audit-first console can then drill from a scheduled run
//! into the chat-style transcript. When the writer is `None` the
//! executor preserves the v0.12.0 behaviour and returns
//! `session_id: None`.

use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_runtime::{run_to_completion, RuntimeContext};

use crate::executor::{ExecutionOutcome, JobExecutor};
use crate::job::ScheduledJob;

const OUTPUT_PREVIEW_MAX: usize = 500;

/// Persist a synthetic session for one scheduled-job run.
///
/// The writer creates a session row, persists the messages produced by
/// the agent loop, and returns the new `session_id` so the
/// `JobExecutor` can hand it back inside [`ExecutionOutcome`]. The
/// `audit-first` console joins `scheduled_job_runs.session_id` →
/// `sessions.id` to render the scheduler-driven transcript.
///
/// The writer is responsible for choosing a stable `user_id` for
/// scheduled runs (production wires a synthetic "scheduler:<`job_id`>"
/// user); the trait takes only what's strictly required.
#[async_trait]
pub trait ScheduledSessionWriter: Send + Sync {
    /// Create a session, persist `new_messages`, and return the
    /// `session_id`. Returns a string-level error so the executor can
    /// surface it back through `JobExecutor::execute`'s error channel.
    async fn create_and_record(
        &self,
        job: &ScheduledJob,
        prompt: &str,
        new_messages: &[LlmMessage],
    ) -> Result<String, String>;
}

pub struct RuntimeJobExecutor {
    ctx: Arc<RuntimeContext>,
    session_writer: Option<Arc<dyn ScheduledSessionWriter>>,
}

impl RuntimeJobExecutor {
    /// Construct an executor that does NOT persist a per-run session.
    /// `ExecutionOutcome.session_id` will be `None`. Equivalent to the
    /// v0.12.0 constructor — preserved so existing callers don't need
    /// to thread an `Option<Arc<...>>` they wouldn't use.
    #[must_use]
    pub fn new(ctx: Arc<RuntimeContext>) -> Self {
        Self {
            ctx,
            session_writer: None,
        }
    }

    /// v0.12.1: opt into per-run synthetic sessions. When `Some`, every
    /// successful run gets a session row + messages persisted via the
    /// writer; `ExecutionOutcome.session_id` carries the new id.
    #[must_use]
    pub fn with_session_writer(mut self, writer: Arc<dyn ScheduledSessionWriter>) -> Self {
        self.session_writer = Some(writer);
        self
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

        let history = vec![LlmMessage::user(prompt)];
        // L3 attribution: a scheduled run has no chat session at run time (the
        // synthetic session, if any, is written AFTER the run by the writer
        // below), so attribute `token_usage` to a stable per-job label. This
        // also aggregates a job's cost across all its runs. `token_usage`'s
        // `session_id` is an un-keyed TEXT column, so a synthetic id is fine.
        let attribution = scheduled_attribution_id(&job.id);
        let ctx = self
            .ctx
            .with_attribution(Some(attribution.clone()), Some(attribution));
        let outcome = run_to_completion(&ctx, history, tokio_util::sync::CancellationToken::new())
            .await
            .map_err(|e| format!("runtime: {e}"))?;

        // v0.12.1: optionally persist a synthetic session so the
        // audit-first console can drill into the transcript. We do this
        // AFTER the runtime returns so a writer failure can't cancel an
        // already-completed agent run — it surfaces as the JobRun error
        // and the audit row instead. Writer failure does not roll back
        // the agent run (it already ran), but it does flip the run to
        // failed in the JobRun row so the operator notices.
        let session_id = if let Some(writer) = &self.session_writer {
            Some(
                writer
                    .create_and_record(job, prompt, &outcome.new_messages)
                    .await
                    .map_err(|e| format!("session writer: {e}"))?,
            )
        } else {
            None
        };

        Ok(ExecutionOutcome {
            output_preview: truncate_preview(&outcome.reply_text),
            session_id,
        })
    }
}

/// Per-job attribution label for `token_usage`. A scheduled run has no chat
/// session at run time, so usage is attributed to this `scheduler:<job_id>`
/// label (used as both session and user id), which aggregates a job's cost
/// across all its runs. NB when a [`ScheduledSessionWriter`] is configured it
/// persists a separate synthetic `sess_*` session AFTER the run; `token_usage`
/// is deliberately keyed by job, not that per-run session, so a per-session cost
/// view would NOT join to these rows (per-job aggregation was chosen over
/// per-run linkage). The label is an opaque key; keep the `scheduler:` prefix
/// stable.
fn scheduled_attribution_id(job_id: &str) -> String {
    format!("scheduler:{job_id}")
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
    use parking_lot::Mutex;
    use std::sync::Arc;
    use xiaoguai_agent::{AgentConfig, Toolbox};
    use xiaoguai_llm::{LlmBackend, MockBackend};

    use crate::job::ScheduledJob;
    use crate::trigger::Trigger;

    fn make_ctx(reply: &str) -> Arc<RuntimeContext> {
        let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response(reply));
        Arc::new(RuntimeContext::new(
            backend,
            Arc::new(Toolbox::new()),
            AgentConfig::new("mock-model"),
        ))
    }

    fn make_job(prompt: &serde_json::Value) -> ScheduledJob {
        ScheduledJob::new(
            "j1",
            "j1",
            Trigger::interval(60).unwrap(),
            serde_json::json!({ "prompt": prompt }),
        )
    }

    struct RecordingSessionWriter {
        calls: Mutex<Vec<(String, String, usize)>>,
        next_id: &'static str,
        fail: bool,
    }

    #[async_trait]
    impl ScheduledSessionWriter for RecordingSessionWriter {
        async fn create_and_record(
            &self,
            job: &ScheduledJob,
            prompt: &str,
            new_messages: &[LlmMessage],
        ) -> Result<String, String> {
            self.calls
                .lock()
                .push((job.id.clone(), prompt.to_string(), new_messages.len()));
            if self.fail {
                return Err("writer boom".into());
            }
            Ok(self.next_id.to_string())
        }
    }

    #[tokio::test]
    async fn execute_returns_reply_text_as_preview() {
        let exec = RuntimeJobExecutor::new(make_ctx("hello scheduled world"));
        let job = make_job(&serde_json::Value::String("ping".into()));
        let outcome = exec.execute(&job, 1).await.unwrap();
        assert_eq!(outcome.output_preview, "hello scheduled world");
        assert!(outcome.session_id.is_none());
    }

    #[tokio::test]
    async fn execute_errors_when_prompt_missing() {
        let exec = RuntimeJobExecutor::new(make_ctx("never reached"));
        let job = make_job(&serde_json::Value::Null);
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

    #[test]
    fn scheduled_attribution_id_is_job_scoped() {
        assert_eq!(scheduled_attribution_id("job-42"), "scheduler:job-42");
    }

    #[test]
    fn scheduled_attribution_id_treats_input_as_opaque() {
        // Empty and colon-bearing ids must not panic or be re-parsed — the
        // label is an opaque key matched whole.
        assert_eq!(scheduled_attribution_id(""), "scheduler:");
        assert_eq!(scheduled_attribution_id("a:b"), "scheduler:a:b");
    }

    #[tokio::test]
    async fn execute_with_session_writer_returns_session_id() {
        let writer = Arc::new(RecordingSessionWriter {
            calls: Mutex::new(Vec::new()),
            next_id: "sess_42",
            fail: false,
        });
        let exec = RuntimeJobExecutor::new(make_ctx("ok"))
            .with_session_writer(writer.clone() as Arc<dyn ScheduledSessionWriter>);
        let job = make_job(&serde_json::Value::String("hello".into()));
        let outcome = exec.execute(&job, 1).await.unwrap();
        assert_eq!(outcome.session_id.as_deref(), Some("sess_42"));
        let calls = writer.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "j1");
        assert_eq!(calls[0].1, "hello");
        // new_messages slice contains at least the user prompt + the assistant reply.
        assert!(
            calls[0].2 >= 2,
            "expected ≥2 new messages, got {}",
            calls[0].2
        );
    }

    #[tokio::test]
    async fn execute_surfaces_session_writer_error() {
        let writer = Arc::new(RecordingSessionWriter {
            calls: Mutex::new(Vec::new()),
            next_id: "unused",
            fail: true,
        });
        let exec = RuntimeJobExecutor::new(make_ctx("ok"))
            .with_session_writer(writer as Arc<dyn ScheduledSessionWriter>);
        let job = make_job(&serde_json::Value::String("hello".into()));
        let err = exec.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("session writer"));
    }
}
