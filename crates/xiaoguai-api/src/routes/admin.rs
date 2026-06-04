//! `/v1/admin/*` — administrative endpoints.
//!
//! `GET /v1/admin/audit` is backed by the HMAC-chained audit log
//! (`xiaoguai-audit::SqliteAuditSink` in production via the [`AuditReader`]
//! bridge).
//!
//! Under DEC-033 these endpoints carry no RBAC of their own — when
//! `AppState.auth` is set every caller is the single static owner; when it
//! is unset (dev mode) they are reachable without a credential, the same
//! trust model as the rest of the API.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use xiaoguai_eval::EvalReport;

use crate::audit::{AuditEntryView, VerifyReport};
use crate::error::{ApiError, ApiResult};
use crate::eval::{
    CaseFromSessionRequest, CaseFromSessionResponse, EvalServiceError, EvalSuiteListItem,
    RunEvalRequest,
};
use crate::scheduler::{
    NlJobCompileError, ScheduledJobUpsertError, ScheduledJobsReadError, WebhookTokenAdminError,
    WebhookTokenRecord,
};
use crate::state::AppState;
use crate::today::{TodayItem, TodayKind, TodayQuery};

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;
const DEFAULT_TODAY_LIMIT: i64 = 50;
const MAX_TODAY_LIMIT: i64 = 500;

#[derive(Debug, Deserialize, Default)]
pub struct ListAuditQuery {
    pub limit: Option<i64>,
    /// RFC 3339 timestamp; inclusive lower bound.
    pub since: Option<DateTime<Utc>>,
    /// RFC 3339 timestamp; inclusive upper bound.
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct VerifyAuditResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken_at: Option<i64>,
}

/// v0.6.5 — chain-integrity verification surfaced for admin / monitoring.
/// 200 `{"ok": true, "verified_count": N}` on success;
/// 200 `{"ok": false, "broken_at": rowid}` when the chain breaks (the
/// response is HTTP 200 on purpose so dashboards can scrape it; the
/// `ok` flag is the alerting signal).
/// 503 when no verifier is wired.
///
/// # Errors
/// Returns an error if the verifier is not wired or the query fails.
pub async fn verify_audit(State(state): State<AppState>) -> ApiResult<Json<VerifyAuditResponse>> {
    let verifier = state
        .audit_verifier
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("audit verifier not wired".into()))?;
    let report = verifier
        .verify_tenant(xiaoguai_audit::OWNER_TENANT_ID)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("audit verify: {e}")))?;
    let body = match report {
        VerifyReport::Ok { verified_count } => VerifyAuditResponse {
            ok: true,
            verified_count: Some(verified_count),
            broken_at: None,
        },
        VerifyReport::Broken { broken_at } => VerifyAuditResponse {
            ok: false,
            verified_count: None,
            broken_at: Some(broken_at),
        },
    };
    Ok(Json(body))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListTodayQuery {
    pub limit: Option<i64>,
    /// RFC 3339 timestamp; inclusive lower bound on item `ts`.
    pub since: Option<DateTime<Utc>>,
    /// `chat` / `im` / `scheduled` — filters to a single source.
    pub kind: Option<TodayKind>,
}

/// v0.11.1 — audit-first console substrate. Merges the three most-recent
/// streams (chat / IM / scheduled) into one timeline sorted by ts desc.
/// Behind the same admin auth stack as the rest of `/v1/admin/*`.
///
/// # Errors
/// Returns an error if the today reader is not wired or the query fails.
pub async fn list_today(
    State(state): State<AppState>,
    Query(q): Query<ListTodayQuery>,
) -> ApiResult<Json<Vec<TodayItem>>> {
    let reader = state
        .today
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("today reader not wired".into()))?;
    let limit = q
        .limit
        .unwrap_or(DEFAULT_TODAY_LIMIT)
        .clamp(1, MAX_TODAY_LIMIT);
    let items = reader
        .list(TodayQuery {
            limit,
            since: q.since,
            kind: q.kind,
        })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("today list: {e}")))?;
    Ok(Json(items))
}

/// # Errors
/// Returns an error if the audit reader is not wired or the query fails.
pub async fn list_audit(
    State(state): State<AppState>,
    Query(q): Query<ListAuditQuery>,
) -> ApiResult<Json<Vec<AuditEntryView>>> {
    let reader = state
        .audit
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("audit reader not wired".into()))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = reader
        .list(xiaoguai_audit::OWNER_TENANT_ID, q.since, q.until, limit)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("audit list: {e}")))?;
    Ok(Json(rows))
}

// ----------------------------------------------------------------------
// v0.11.2 — eval pane endpoints.
// ----------------------------------------------------------------------

/// `GET /v1/admin/eval/suites` — enumerate suites available on disk so
/// the console can render a clickable left-hand list.
///
/// # Errors
/// Returns an error if the eval service is not wired or the directory cannot be read.
pub async fn list_eval_suites(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<EvalSuiteListItem>>> {
    let svc = state
        .eval
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("eval service not wired".into()))?;
    let items = svc.list_suites().map_err(eval_err_to_api)?;
    Ok(Json(items))
}

/// `POST /v1/admin/eval/run` — execute a suite synchronously and
/// return the full report. The per-request caps live in
/// `eval::MAX_CASES_PER_RUN` + `eval::MAX_RUN_DURATION`.
///
/// # Errors
/// Returns an error if the eval service is not wired, the suite is too large, or it times out.
pub async fn run_eval_suite(
    State(state): State<AppState>,
    Json(req): Json<RunEvalRequest>,
) -> ApiResult<Json<EvalReport>> {
    let svc = state
        .eval
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("eval service not wired".into()))?;
    let report = svc.run_suite(&req).await.map_err(eval_err_to_api)?;
    Ok(Json(report))
}

/// `POST /v1/admin/eval/case-from-session` — project a production
/// `sessions.id` into a ready-to-edit `EvalCase` YAML the operator
/// pastes into a new `.eval.yaml` file. Does **not** write to disk; the
/// caller reviews + commits.
///
/// # Errors
/// Returns an error if the eval service is not wired or the session is not found.
pub async fn eval_case_from_session(
    State(state): State<AppState>,
    Json(req): Json<CaseFromSessionRequest>,
) -> ApiResult<Json<CaseFromSessionResponse>> {
    let svc = state
        .eval
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("eval service not wired".into()))?;
    let resp = svc.case_from_session(&req).await.map_err(eval_err_to_api)?;
    Ok(Json(resp))
}

/// `POST /v1/admin/scheduler/webhooks/:route_id` — push a reactive
/// trigger event into the scheduler. Body is opaque JSON forwarded
/// into the audit row's `details.trigger` field.
///
/// Returns 202 with `{ "delivered": N }` when at least one job was
/// notified; 404 when no jobs are bound to `route_id`; 503 when the
/// scheduler isn't wired in this process.
///
/// Per-tenant API tokens (so external integrators can hit the endpoint
/// without an admin bearer) land in v0.12.1 — today the route uses the
/// existing admin bearer/Casbin guard.
///
/// # Errors
/// Returns an error if the webhook pusher is not wired, no jobs matched, or the push fails.
pub async fn scheduler_webhook(
    State(state): State<AppState>,
    Path(route_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    let pusher = state
        .webhook_pusher
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("scheduler webhook not wired".into()))?;
    let delivered = pusher
        .push(&route_id, body)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("webhook push: {e}")))?;
    if delivered == 0 {
        return Err(ApiError::NotFound);
    }
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(json!({ "delivered": delivered })),
    ))
}

// ----------------------------------------------------------------------
// v0.12.1 — natural-language scheduled-job definition.
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CompileJobRequest {
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct CompileJobResponse {
    /// Fully-populated `ScheduledJob` (JSON shape mirrors
    /// `xiaoguai_scheduler::ScheduledJob`). Surfaced to the operator
    /// for review before they POST to `/v1/admin/scheduler/jobs`.
    pub suggested_job: serde_json::Value,
    /// Short human-readable explanation of how the LLM interpreted the
    /// description. Shown in the admin-ui Scheduler pane.
    pub rationale: String,
}

/// `POST /v1/admin/scheduler/jobs/compile` — turn a free-form
/// description ("每天 8 点扫 r/LocalLLaMA + HN 推 Telegram") into a
/// ready-to-review `ScheduledJob` row. Does NOT persist; the operator
/// reviews and then POSTs to `/v1/admin/scheduler/jobs`.
///
/// # Errors
/// Returns an error if the description is empty, the compiler is not wired, or the LLM fails.
pub async fn scheduler_compile_job(
    State(state): State<AppState>,
    Json(req): Json<CompileJobRequest>,
) -> ApiResult<Json<CompileJobResponse>> {
    if req.description.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "description must not be empty".into(),
        ));
    }
    let compiler = state
        .nl_job_compiler
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("nl job compiler not wired".into()))?;
    let (suggested_job, rationale) = compiler
        .compile(&req.description)
        .await
        .map_err(nl_compile_err_to_api)?;
    Ok(Json(CompileJobResponse {
        suggested_job,
        rationale,
    }))
}

/// `POST /v1/admin/scheduler/jobs` — upsert a `ScheduledJob` row.
/// Body shape mirrors `xiaoguai_scheduler::ScheduledJob`. Returns 201
/// on success, 400 on invalid payload, 503 when the scheduler isn't
/// wired in this process.
///
/// # Errors
/// Returns an error if the upserter is not wired or the job payload is invalid.
pub async fn scheduler_upsert_job(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    let upserter = state
        .job_upserter
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("scheduler job upserter not wired".into()))?;
    // Pull the id back to return it in the 201 response so callers don't
    // have to re-parse the request body.
    let id = body
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    upserter.upsert(body).await.map_err(upsert_err_to_api)?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(json!({ "id": id.unwrap_or_default() })),
    ))
}

// ----------------------------------------------------------------------
// v0.12.x.1 — webhook token admin + Scheduler-pane jobs reader.
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    pub route_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListTokensQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub token: String,
    pub route_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<WebhookTokenRecord> for TokenResponse {
    fn from(r: WebhookTokenRecord) -> Self {
        Self {
            token: r.token,
            route_id: r.route_id,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
    }
}

/// `POST /v1/admin/scheduler/tokens` — mint a new webhook token bound to
/// `route_id`. The token is returned exactly once in the response body;
/// the operator must capture it immediately.
///
/// # Errors
/// Returns an error if the token admin is not wired, inputs are empty, or creation fails.
pub async fn scheduler_create_token(
    State(state): State<AppState>,
    Json(req): Json<CreateTokenRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<TokenResponse>)> {
    let admin = state
        .webhook_token_admin
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("webhook token admin not wired".into()))?;
    if req.route_id.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "route_id must not be empty".into(),
        ));
    }
    let row = admin
        .create(req.route_id.trim())
        .await
        .map_err(token_admin_err_to_api)?;
    Ok((axum::http::StatusCode::CREATED, Json(row.into())))
}

/// `GET /v1/admin/scheduler/tokens?limit=...` — list tokens.
///
/// # Errors
/// Returns an error if the token admin is not wired or the query fails.
pub async fn scheduler_list_tokens(
    State(state): State<AppState>,
    Query(q): Query<ListTokensQuery>,
) -> ApiResult<Json<Vec<TokenResponse>>> {
    let admin = state
        .webhook_token_admin
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("webhook token admin not wired".into()))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = admin.list(limit).await.map_err(token_admin_err_to_api)?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// `DELETE /v1/admin/scheduler/tokens/:token` — revoke a webhook token.
///
/// # Errors
/// Returns an error if the token admin is not wired, the token is not found, or revocation fails.
#[allow(
    clippy::needless_pass_by_value,
    reason = "Axum Path extractor requires owned String"
)]
pub async fn scheduler_revoke_token(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    let admin = state
        .webhook_token_admin
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("webhook token admin not wired".into()))?;
    admin.revoke(&token).await.map_err(token_admin_err_to_api)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, Default)]
pub struct ListScheduledJobsQuery {
    pub limit: Option<i64>,
}

/// `GET /v1/admin/scheduler/jobs` — enumerate scheduled jobs for the
/// admin-ui Scheduler pane's Jobs tab. Returns the narrow
/// `ScheduledJobSummary` shape; drill-in (full row) is a separate
/// future endpoint.
///
/// # Errors
/// Returns an error if the jobs reader is not wired or the query fails.
pub async fn scheduler_list_jobs(
    State(state): State<AppState>,
    Query(q): Query<ListScheduledJobsQuery>,
) -> ApiResult<Json<Vec<crate::scheduler::ScheduledJobSummary>>> {
    let reader = state
        .scheduler_jobs_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("scheduled jobs reader not wired".into()))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = reader.list(limit).await.map_err(jobs_read_err_to_api)?;
    Ok(Json(rows))
}

/// `POST /v1/admin/scheduler/jobs/:id/fire-now` — fire one scheduled
/// job out-of-band (regardless of `next_fire_at`). Returns 202; the
/// run completes asynchronously and shows up in the next refresh of
/// the Today pane.
///
/// # Errors
/// Returns an error if the jobs reader is not wired, the job is not found, or the fire fails.
pub async fn scheduler_fire_now(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> ApiResult<(axum::http::StatusCode, Json<serde_json::Value>)> {
    let reader = state
        .scheduler_jobs_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("scheduled jobs reader not wired".into()))?;
    reader
        .fire_now(&job_id)
        .await
        .map_err(jobs_read_err_to_api)?;
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(json!({ "fired": job_id })),
    ))
}

fn token_admin_err_to_api(e: WebhookTokenAdminError) -> ApiError {
    match e {
        WebhookTokenAdminError::NotFound(_) => ApiError::NotFound,
        WebhookTokenAdminError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        WebhookTokenAdminError::Backend(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "match destructures the error by value"
)]
fn jobs_read_err_to_api(e: ScheduledJobsReadError) -> ApiError {
    match e {
        ScheduledJobsReadError::NotFound(_) => ApiError::NotFound,
        ScheduledJobsReadError::Backend(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}

fn nl_compile_err_to_api(e: NlJobCompileError) -> ApiError {
    match e {
        NlJobCompileError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        NlJobCompileError::Unparseable(msg) => ApiError::BadRequest(msg),
        NlJobCompileError::Backend(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}

fn upsert_err_to_api(e: ScheduledJobUpsertError) -> ApiError {
    match e {
        ScheduledJobUpsertError::InvalidJob(msg) => ApiError::BadRequest(msg),
        ScheduledJobUpsertError::Repository(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}

fn eval_err_to_api(e: EvalServiceError) -> ApiError {
    match e {
        EvalServiceError::NotFound(msg) => ApiError::BadRequest(msg),
        EvalServiceError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        EvalServiceError::SuiteTooLarge { .. } => ApiError::PayloadTooLarge(e.to_string()),
        EvalServiceError::SuiteTimedOut { .. } => ApiError::GatewayTimeout(e.to_string()),
        EvalServiceError::Backend(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}
