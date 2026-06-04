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

// ----------------------------------------------------------------------
// v0.12.x.1 — webhook tokens.
// ----------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum WebhookTokenError {
    #[error("backend: {0}")]
    Backend(String),
}

/// Validate a webhook token against `(token, route_id)`. Returns
/// `Ok(true)` when the token is valid for the route; `Ok(false)` on a
/// mismatch (unknown token, or token bound to a different route).
///
/// This trait fronts the `scheduler_webhook_tokens` table — see
/// `crates/xiaoguai-storage/migrations/0008_scheduler_webhook_tokens.sql`.
/// The production impl (`PgWebhookTokenValidator` in `xiaoguai-core`)
/// also best-effort updates `last_used_at`; failures there are logged
/// but do not block the push (the audit row is the source of truth).
#[async_trait]
pub trait WebhookTokenValidator: Send + Sync {
    async fn validate(&self, token: &str, route_id: &str) -> Result<bool, WebhookTokenError>;
}

#[derive(Debug, Error)]
pub enum WebhookTokenAdminError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("backend: {0}")]
    Backend(String),
}

/// One row out of `scheduler_webhook_tokens`. Surfaced to admin
/// endpoints (list + create + revoke) — the boundary is kept as a
/// plain struct rather than a `Value` because admin routes always
/// know the shape and want strict typing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookTokenRecord {
    pub token: String,
    pub route_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Admin-side CRUD for `scheduler_webhook_tokens`. Backs
/// `/v1/admin/scheduler/tokens` (list / create / revoke). Separate from
/// [`WebhookTokenValidator`] so the read-path on the public webhook
/// route doesn't drag the admin surface in.
#[async_trait]
pub trait WebhookTokenAdmin: Send + Sync {
    async fn create(&self, route_id: &str)
        -> Result<WebhookTokenRecord, WebhookTokenAdminError>;
    /// List tokens. Returns at most `limit` rows (default 100, max 1000).
    async fn list(&self, limit: i64) -> Result<Vec<WebhookTokenRecord>, WebhookTokenAdminError>;
    /// Revoke (delete) a token. Returns `NotFound` if no row matched.
    async fn revoke(&self, token: &str) -> Result<(), WebhookTokenAdminError>;
}

// ----------------------------------------------------------------------
// v0.12.x.1 — scheduled-jobs reader (admin pane).
// ----------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ScheduledJobsReadError {
    #[error("backend: {0}")]
    Backend(String),
    #[error("not found: {0}")]
    NotFound(String),
}

/// Summary row for the admin-ui Scheduler pane's Jobs tab. Kept narrow
/// so the wire shape stays small — the pane's drill-in fetches the
/// full job via a separate (deferred) endpoint when needed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScheduledJobSummary {
    pub id: String,
    pub name: String,
    pub trigger_summary: String,
    pub enabled: bool,
    pub last_fire_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_fire_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Read scheduled-job rows for the admin pane + fire one manually.
/// `fire_now` returns `Ok(())` when the runner has accepted the
/// out-of-band fire (the actual execution is async; the operator's UI
/// updates on the next refresh).
#[async_trait]
pub trait ScheduledJobsReader: Send + Sync {
    async fn list(&self, limit: i64) -> Result<Vec<ScheduledJobSummary>, ScheduledJobsReadError>;
    async fn fire_now(&self, job_id: &str) -> Result<(), ScheduledJobsReadError>;
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

// ----------------------------------------------------------------------
// v0.12.x.1 — in-memory test helpers.
// ----------------------------------------------------------------------

/// Static token validator: returns `true` only when the
/// `(token, route_id)` pair matches the canned binding.
pub struct StaticWebhookTokenValidator {
    pub token: String,
    pub route_id: String,
}

#[async_trait]
impl WebhookTokenValidator for StaticWebhookTokenValidator {
    async fn validate(&self, token: &str, route_id: &str) -> Result<bool, WebhookTokenError> {
        Ok(token == self.token && route_id == self.route_id)
    }
}

/// In-memory `WebhookTokenAdmin` for route tests. Generates tokens as
/// `tok-N`; preserves the chronological order in `list`.
#[derive(Default)]
pub struct InMemoryWebhookTokenAdmin {
    rows: parking_lot::Mutex<Vec<WebhookTokenRecord>>,
    counter: parking_lot::Mutex<u32>,
}

#[async_trait]
impl WebhookTokenAdmin for InMemoryWebhookTokenAdmin {
    async fn create(
        &self,
        route_id: &str,
    ) -> Result<WebhookTokenRecord, WebhookTokenAdminError> {
        if route_id.is_empty() {
            return Err(WebhookTokenAdminError::InvalidArgument(
                "route_id required".into(),
            ));
        }
        let mut c = self.counter.lock();
        *c += 1;
        let token = format!("tok-{}", *c);
        drop(c);
        let row = WebhookTokenRecord {
            token: token.clone(),
            route_id: route_id.to_string(),
            created_at: chrono::Utc::now(),
            last_used_at: None,
        };
        self.rows.lock().push(row.clone());
        Ok(row)
    }
    async fn list(&self, limit: i64) -> Result<Vec<WebhookTokenRecord>, WebhookTokenAdminError> {
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        let rows = self.rows.lock();
        Ok(rows.iter().take(limit).cloned().collect())
    }
    async fn revoke(&self, token: &str) -> Result<(), WebhookTokenAdminError> {
        let mut rows = self.rows.lock();
        let before = rows.len();
        rows.retain(|r| r.token != token);
        if rows.len() == before {
            return Err(WebhookTokenAdminError::NotFound(token.into()));
        }
        Ok(())
    }
}

/// Static jobs reader for tests: returns the canned summary list and
/// records `fire_now` calls.
#[derive(Default)]
pub struct StaticScheduledJobsReader {
    pub jobs: Vec<ScheduledJobSummary>,
    pub fire_calls: parking_lot::Mutex<Vec<String>>,
}

#[async_trait]
impl ScheduledJobsReader for StaticScheduledJobsReader {
    async fn list(&self, limit: i64) -> Result<Vec<ScheduledJobSummary>, ScheduledJobsReadError> {
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(self.jobs.iter().take(limit).cloned().collect())
    }
    async fn fire_now(&self, job_id: &str) -> Result<(), ScheduledJobsReadError> {
        if !self.jobs.iter().any(|j| j.id == job_id) {
            return Err(ScheduledJobsReadError::NotFound(job_id.into()));
        }
        self.fire_calls.lock().push(job_id.into());
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
        let (j, r) = c.compile("anything").await.unwrap();
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

    #[tokio::test]
    async fn static_token_validator_matches_only_exact_pair() {
        let v = StaticWebhookTokenValidator {
            token: "secret".into(),
            route_id: "deploy".into(),
        };
        assert!(v.validate("secret", "deploy").await.unwrap());
        assert!(!v.validate("secret", "other").await.unwrap());
        assert!(!v.validate("nope", "deploy").await.unwrap());
    }

    #[tokio::test]
    async fn in_memory_token_admin_create_list_revoke() {
        let admin = InMemoryWebhookTokenAdmin::default();
        let row = admin.create("deploy").await.unwrap();
        assert_eq!(row.route_id, "deploy");
        assert!(row.token.starts_with("tok-"));
        let _ = admin.create("build").await.unwrap();
        let _ = admin.create("deploy").await.unwrap();
        let all = admin.list(100).await.unwrap();
        assert_eq!(all.len(), 3);
        admin.revoke(&row.token).await.unwrap();
        let err = admin.revoke(&row.token).await.unwrap_err();
        assert!(matches!(err, WebhookTokenAdminError::NotFound(_)));
    }

    #[tokio::test]
    async fn in_memory_token_admin_rejects_empty_route() {
        let admin = InMemoryWebhookTokenAdmin::default();
        let err = admin.create("").await.unwrap_err();
        assert!(matches!(err, WebhookTokenAdminError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn static_jobs_reader_list_and_fire_now() {
        let job = ScheduledJobSummary {
            id: "j1".into(),
            name: "daily-scan".into(),
            trigger_summary: "cron `0 0 8 * * *`".into(),
            enabled: true,
            last_fire_at: None,
            next_fire_at: None,
        };
        let reader = StaticScheduledJobsReader {
            jobs: vec![job],
            fire_calls: parking_lot::Mutex::new(Vec::new()),
        };
        let got = reader.list(100).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "j1");
        reader.fire_now("j1").await.unwrap();
        assert_eq!(reader.fire_calls.lock().as_slice(), &["j1".to_string()]);
        let err = reader.fire_now("nope").await.unwrap_err();
        assert!(matches!(err, ScheduledJobsReadError::NotFound(_)));
    }
}
