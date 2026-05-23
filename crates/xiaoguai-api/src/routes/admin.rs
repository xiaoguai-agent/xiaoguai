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

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use xiaoguai_types::{Tenant, TenantStatus};

use crate::audit::AuditEntryView;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;

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
