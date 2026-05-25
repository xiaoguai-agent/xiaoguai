//! `POST /v1/outcomes` + `GET /v1/outcomes/summary` + `GET /v1/outcomes/timeseries`
//! — v1.2.4 outcome telemetry route handlers.
//!
//! All three routes sit inside the bearer-gated v1 layer.
//! Agents call `POST /v1/outcomes` authenticated with a tenant token;
//! the admin-ui calls the two GET endpoints authenticated with an admin token.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use xiaoguai_audit::outcomes::OutcomeRange;

use crate::error::{ApiError, ApiResult};
use crate::outcomes::{
    OutcomeWriter, OutcomesReader, OutcomesSummaryResponse, OutcomesTimeseriesResponse,
    RecordOutcomeRequest, RecordOutcomeResponse,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// POST /v1/outcomes
// ---------------------------------------------------------------------------

/// Record a business outcome attribution.  Agents call this after completing
/// a task that produced measurable business value.
pub async fn record_outcome(
    State(state): State<AppState>,
    Json(req): Json<RecordOutcomeRequest>,
) -> ApiResult<(StatusCode, Json<RecordOutcomeResponse>)> {
    let writer = state
        .outcome_writer
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcome writer not wired".into()))?;

    // Basic input validation at the route level before touching the backend.
    if req.kind.is_empty() {
        return Err(ApiError::InvalidRequest("kind must not be empty".into()));
    }
    if req.agent_name.is_empty() {
        return Err(ApiError::InvalidRequest(
            "agent_name must not be empty".into(),
        ));
    }
    if req.value < 0.0 {
        return Err(ApiError::InvalidRequest(
            "value must be non-negative".into(),
        ));
    }
    if req.tenant_id.is_empty() {
        return Err(ApiError::InvalidRequest(
            "tenant_id must not be empty".into(),
        ));
    }

    writer
        .record(req)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcome record: {e}")))?;

    Ok((
        StatusCode::CREATED,
        Json(RecordOutcomeResponse { ok: true }),
    ))
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/summary
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct SummaryQuery {
    pub tenant_id: String,
    /// `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`.
    pub range: Option<String>,
}

/// Aggregated ROI summary — one bucket per outcome kind.  Backs the four
/// summary cards in the admin-ui Outcomes pane.
pub async fn outcomes_summary(
    State(state): State<AppState>,
    Query(q): Query<SummaryQuery>,
) -> ApiResult<Json<OutcomesSummaryResponse>> {
    let reader = state
        .outcomes_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcomes reader not wired".into()))?;

    if q.tenant_id.is_empty() {
        return Err(ApiError::InvalidRequest("tenant_id is required".into()));
    }

    let range_str = q.range.as_deref().unwrap_or("30d");
    let range = OutcomeRange::from_shorthand(range_str)
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

    let summary = reader
        .summary(&q.tenant_id, range)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcomes summary: {e}")))?;

    Ok(Json(OutcomesSummaryResponse {
        tenant_id: q.tenant_id,
        range: range_str.to_owned(),
        summary,
    }))
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/timeseries
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct TimeseriesQuery {
    pub tenant_id: String,
    /// `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`.
    pub range: Option<String>,
    /// Optional kind filter: `"revenue_usd"` | `"hours_saved"` | …
    pub kind: Option<String>,
}

/// Daily time-series breakdown — used by the bar chart in the Outcomes pane.
pub async fn outcomes_timeseries(
    State(state): State<AppState>,
    Query(q): Query<TimeseriesQuery>,
) -> ApiResult<Json<OutcomesTimeseriesResponse>> {
    let reader = state
        .outcomes_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcomes reader not wired".into()))?;

    if q.tenant_id.is_empty() {
        return Err(ApiError::InvalidRequest("tenant_id is required".into()));
    }

    let range_str = q.range.as_deref().unwrap_or("30d");
    let range = OutcomeRange::from_shorthand(range_str)
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

    let days = reader
        .timeseries(&q.tenant_id, q.kind.as_deref(), range)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcomes timeseries: {e}")))?;

    Ok(Json(OutcomesTimeseriesResponse {
        tenant_id: q.tenant_id,
        range: range_str.to_owned(),
        days,
    }))
}
