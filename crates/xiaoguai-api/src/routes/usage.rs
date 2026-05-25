//! `GET /v1/usage` — token-usage aggregation endpoint (v1.1.1).
//!
//! Admin-bearer-gated like the rest of `/v1/*`; production callers come
//! from the admin-ui Usage pane and the Today pane's "Token usage (24h)"
//! summary card. The handler is a thin wrapper around
//! [`crate::usage::UsageReader`] — all aggregation work lives in the
//! reader trait so the route stays storage-agnostic.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::usage::{UsageGroupBy, UsageQuery, UsageReport};

#[derive(Debug, Deserialize, Default)]
pub struct ListUsageQuery {
    /// Restrict aggregation to a single tenant. `None` = cross-tenant
    /// admin view.
    pub tenant_id: Option<String>,
    /// RFC 3339 timestamp; inclusive lower bound on `token_usage.ts`.
    pub since: Option<DateTime<Utc>>,
    /// RFC 3339 timestamp; inclusive upper bound on `token_usage.ts`.
    pub until: Option<DateTime<Utc>>,
    /// `day` / `provider` / `model`. Defaults to `day`.
    pub group_by: Option<UsageGroupBy>,
}

/// # Errors
/// Returns an error if the usage reader is not wired, the date range is invalid, or the query fails.
pub async fn list_usage(
    State(state): State<AppState>,
    Query(q): Query<ListUsageQuery>,
) -> ApiResult<Json<UsageReport>> {
    let reader = state
        .usage_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("usage reader not wired".into()))?;
    if let (Some(since), Some(until)) = (q.since, q.until) {
        if since > until {
            return Err(ApiError::InvalidRequest("since must be <= until".into()));
        }
    }
    let report = reader
        .aggregate(UsageQuery {
            tenant_id: q.tenant_id,
            since: q.since,
            until: q.until,
            group_by: q.group_by.unwrap_or_default(),
        })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("usage aggregate: {e}")))?;
    Ok(Json(report))
}
