//! `/v1/admin/*` — cross-tenant administrative endpoints.
//!
//! v0.6.3 added the tenant directory listing. v0.6.4 adds
//! `GET /v1/admin/audit?tenant_id=...` backed by the HMAC-chained audit
//! log (`xiaoguai-audit::PgAuditSink` in production via the
//! [`AuditReader`] bridge).
//!
//! All admin endpoints are gated by the Casbin policy
//! (`system_admin, *, *, *`) when `AppState.authz` is `Some(...)`.
//! When `authz` is `None` (dev mode) the endpoints are reachable by
//! any caller — same trust model as the rest of the API in dev.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use xiaoguai_eval::EvalReport;
use xiaoguai_types::{Tenant, TenantStatus};

use crate::audit::{AuditEntryView, VerifyReport};
use crate::error::{ApiError, ApiResult};
use crate::eval::{
    CaseFromSessionRequest, CaseFromSessionResponse, EvalServiceError, EvalSuiteListItem,
    RunEvalRequest,
};
use crate::state::AppState;
use crate::today::{TodayItem, TodayKind, TodayQuery};

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;
const DEFAULT_TODAY_LIMIT: i64 = 50;
const MAX_TODAY_LIMIT: i64 = 500;

#[derive(Debug, Deserialize, Default)]
pub struct ListTenantsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListAuditQuery {
    /// Required. The audit chain is per-tenant; cross-tenant listing
    /// would need a separate endpoint with stricter policy.
    pub tenant_id: Option<String>,
    pub limit: Option<i64>,
    /// RFC 3339 timestamp; inclusive lower bound.
    pub since: Option<DateTime<Utc>>,
    /// RFC 3339 timestamp; inclusive upper bound.
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct TenantResponse {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub status: TenantStatus,
}

impl From<Tenant> for TenantResponse {
    fn from(t: Tenant) -> Self {
        Self {
            id: t.id.to_string(),
            name: t.name,
            display_name: t.display_name,
            status: t.status,
        }
    }
}

pub async fn list_tenants(
    State(state): State<AppState>,
    Query(q): Query<ListTenantsQuery>,
) -> ApiResult<Json<Vec<TenantResponse>>> {
    let repo = state.tenants.as_ref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!("tenant repository not wired into AppState"))
    })?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = repo.list(limit, offset).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[derive(Debug, Deserialize, Default)]
pub struct VerifyAuditQuery {
    pub tenant_id: Option<String>,
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
/// 503 when no verifier is wired; 400 when `tenant_id` is missing.
pub async fn verify_audit(
    State(state): State<AppState>,
    Query(q): Query<VerifyAuditQuery>,
) -> ApiResult<Json<VerifyAuditResponse>> {
    let verifier = state
        .audit_verifier
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("audit verifier not wired".into()))?;
    let tenant_id = q
        .tenant_id
        .ok_or_else(|| ApiError::InvalidRequest("tenant_id is required".into()))?;
    if tenant_id.is_empty() {
        return Err(ApiError::InvalidRequest(
            "tenant_id must not be empty".into(),
        ));
    }
    let report = verifier
        .verify_tenant(&tenant_id)
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

pub async fn list_audit(
    State(state): State<AppState>,
    Query(q): Query<ListAuditQuery>,
) -> ApiResult<Json<Vec<AuditEntryView>>> {
    let reader = state
        .audit
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("audit reader not wired".into()))?;
    let tenant_id = q
        .tenant_id
        .ok_or_else(|| ApiError::InvalidRequest("tenant_id is required".into()))?;
    if tenant_id.is_empty() {
        return Err(ApiError::InvalidRequest(
            "tenant_id must not be empty".into(),
        ));
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rows = reader
        .list(&tenant_id, q.since, q.until, limit)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("audit list: {e}")))?;
    Ok(Json(rows))
}

// ----------------------------------------------------------------------
// v0.11.2 — eval pane endpoints.
// ----------------------------------------------------------------------

/// `GET /v1/admin/eval/suites` — enumerate suites available on disk so
/// the console can render a clickable left-hand list.
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

fn eval_err_to_api(e: EvalServiceError) -> ApiError {
    match e {
        EvalServiceError::NotFound(msg) => ApiError::BadRequest(msg),
        EvalServiceError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        EvalServiceError::SuiteTooLarge { .. } => ApiError::PayloadTooLarge(e.to_string()),
        EvalServiceError::SuiteTimedOut { .. } => ApiError::GatewayTimeout(e.to_string()),
        EvalServiceError::Backend(_) => ApiError::Internal(anyhow::anyhow!("{e}")),
    }
}
