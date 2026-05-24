//! Scheduler integration types — kept here (not in `xiaoguai-scheduler`)
//! to avoid a dependency cycle.
//!
//! `xiaoguai-scheduler` already transitively depends on `xiaoguai-api`
//! (via `xiaoguai-im-feishu` → `xiaoguai-im-gateway` → `xiaoguai-api`),
//! so `xiaoguai-api` cannot depend on `xiaoguai-scheduler` directly.
//! Instead `xiaoguai-api` owns the small trait surface it needs from
//! the scheduler — same pattern as [`crate::audit::AuditReader`] and
//! [`crate::today::TodayReader`].
//!
//! `xiaoguai-core` provides the production impls by wrapping the real
//! `xiaoguai_scheduler` types.
//!
//! v0.12.1 adds two more shim traits beside [`WebhookPusher`]:
//!
//! * [`NlJobCompiler`] — backs `POST /v1/admin/scheduler/jobs/compile`.
//!   Production impl ([`xiaoguai-core::scheduler_bridge::LlmNlJobCompiler`])
//!   sends the user's free-form description to the configured
//!   [`xiaoguai_llm::LlmBackend`] together with a strict JSON-schema
//!   prompt and parses the response back into a `ScheduledJob`.
//! * [`ScheduledJobUpserter`] — backs `POST /v1/admin/scheduler/jobs`.
//!   Production impl wraps the `PgJobRepository`. The boundary uses
//!   `serde_json::Value` so the api crate stays free of the
//!   `ScheduledJob` type definition.

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WebhookPushError {
    #[error("source backend: {0}")]
    Backend(String),
}

/// Push a reactive trigger event onto the scheduler's event channel.
///
/// `route_id` identifies the bound (route → job) mapping inside the
/// scheduler. `detail` is opaque JSON that lands in the audit row of
/// every fired job under `details.trigger`.
///
/// Returns the count of jobs that were notified. A return of `Ok(0)`
/// means no jobs are bound to `route_id` — the HTTP handler maps that
/// to 404.
#[async_trait]
pub trait WebhookPusher: Send + Sync {
    async fn push(
        &self,
        route_id: &str,
        detail: serde_json::Value,
    ) -> Result<usize, WebhookPushError>;
}

#[derive(Debug, Error)]
pub enum NlJobCompileError {
    #[error("backend: {0}")]
    Backend(String),
    /// The compiler returned a response that did not parse as a valid
    /// `ScheduledJob`. The HTTP handler maps this to 400 — the user can
    /// retry with a clearer prompt.
    #[error("compiler returned unparseable job: {0}")]
    Unparseable(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// Compile a natural-language job description into a fully-populated
/// `ScheduledJob` row.
///
/// Returns `(suggested_job, rationale)`. `suggested_job` is the JSON
/// representation of a `xiaoguai_scheduler::ScheduledJob`; the HTTP
/// boundary keeps it as `serde_json::Value` so the api crate stays free
/// of the scheduler type definition (see module docs).
///
/// `rationale` is a short human-readable explanation of how the
/// compiler interpreted the description — surfaced to the operator so
/// they can verify before clicking Save.
#[async_trait]
pub trait NlJobCompiler: Send + Sync {
    async fn compile(
        &self,
        description: &str,
        tenant_id: Option<&str>,
    ) -> Result<(serde_json::Value, String), NlJobCompileError>;
}

#[derive(Debug, Error)]
pub enum ScheduledJobUpsertError {
    #[error("invalid job: {0}")]
    InvalidJob(String),
    #[error("repository: {0}")]
    Repository(String),
}

/// Persist a `ScheduledJob` (insert or update) by its `id` field.
///
/// The `job` value must deserialise into `xiaoguai_scheduler::ScheduledJob`.
/// Validation lives in the production impl so the api crate can stay
/// type-agnostic; failures surface as
/// [`ScheduledJobUpsertError::InvalidJob`] which the HTTP handler maps
/// to 400.
#[async_trait]
pub trait ScheduledJobUpserter: Send + Sync {
    async fn upsert(&self, job: serde_json::Value) -> Result<(), ScheduledJobUpsertError>;
}

/// In-memory `NlJobCompiler` for route tests + dev mode. Returns a
/// pre-canned `(job_json, rationale)` pair regardless of input.
pub struct StaticNlJobCompiler {
    pub job: serde_json::Value,
    pub rationale: String,
}

#[async_trait]
impl NlJobCompiler for StaticNlJobCompiler {
    async fn compile(
        &self,
        _description: &str,
        _tenant_id: Option<&str>,
    ) -> Result<(serde_json::Value, String), NlJobCompileError> {
        Ok((self.job.clone(), self.rationale.clone()))
    }
}

/// In-memory `ScheduledJobUpserter` for route tests + dev mode.
/// Records every accepted job so assertions can inspect the call.
#[derive(Default)]
pub struct RecordingJobUpserter {
    pub jobs: parking_lot::Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl ScheduledJobUpserter for RecordingJobUpserter {
    async fn upsert(&self, job: serde_json::Value) -> Result<(), ScheduledJobUpsertError> {
        // Reject obviously invalid payloads early so the route returns 400
        // — production impl does the full `ScheduledJob` deserialize.
        if job.get("id").and_then(serde_json::Value::as_str).is_none() {
            return Err(ScheduledJobUpsertError::InvalidJob(
                "missing string field `id`".into(),
            ));
        }
        self.jobs.lock().push(job);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_compiler_returns_canned_pair() {
        let c = StaticNlJobCompiler {
            job: serde_json::json!({"id": "j1"}),
            rationale: "because".into(),
        };
        let (j, r) = c.compile("anything", None).await.unwrap();
        assert_eq!(j["id"], "j1");
        assert_eq!(r, "because");
    }

    #[tokio::test]
    async fn recording_upserter_rejects_missing_id() {
        let u = RecordingJobUpserter::default();
        let err = u.upsert(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ScheduledJobUpsertError::InvalidJob(_)));
    }

    #[tokio::test]
    async fn recording_upserter_keeps_accepted_jobs() {
        let u = RecordingJobUpserter::default();
        u.upsert(serde_json::json!({"id": "j1"})).await.unwrap();
        u.upsert(serde_json::json!({"id": "j2"})).await.unwrap();
        let jobs = u.jobs.lock();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0]["id"], "j1");
        assert_eq!(jobs[1]["id"], "j2");
    }
}
