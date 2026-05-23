//! `/v1/admin/*` — cross-tenant administrative endpoints.
//!
//! v0.6.3 ships the tenant directory listing only. `/v1/admin/audit`
//! is deferred until a PG-backed audit store exists (xiaoguai-audit
//! currently only chains HMACs in-memory).
//!
//! All admin endpoints are gated by the Casbin policy
//! (`system_admin, *, *, *`) when `AppState.authz` is `Some(...)`.
//! When `authz` is `None` (dev mode) the endpoints are reachable by
//! any caller — same trust model as the rest of the API in dev.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use xiaoguai_types::{Tenant, TenantStatus};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, Default)]
pub struct ListTenantsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
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
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = repo.list(limit, offset).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}
