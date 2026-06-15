//! `POST /v1/outcomes` + `GET /v1/outcomes` (list) + `GET /v1/outcomes/summary`
//! + `GET /v1/outcomes/timeseries` — v1.2.4 outcome telemetry route handlers.
//!
//! All three routes sit inside the bearer-gated v1 layer.
//! Agents call `POST /v1/outcomes` authenticated with a tenant token;
//! the admin-ui calls the two GET endpoints authenticated with an admin token.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use xiaoguai_audit::outcomes::{OutcomeRange, OutcomeRecord};

use crate::error::{ApiError, ApiResult};
use crate::outcomes::{
    OutcomesSummaryResponse, OutcomesTimeseriesResponse, RecordOutcomeRequest,
    RecordOutcomeResponse,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// POST /v1/outcomes
// ---------------------------------------------------------------------------

/// Record a business outcome attribution.  Agents call this after completing
/// a task that produced measurable business value.
///
/// # Errors
/// Returns an error if the outcome writer is not wired, inputs are invalid, or the write fails.
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
// GET /v1/outcomes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListOutcomesQuery {
    /// `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`.
    pub range: Option<String>,
    /// Optional kind filter (e.g. `"revenue_usd"`).
    pub kind: Option<String>,
}

/// Raw outcome records (newest first) backing the Outcomes pane list tab.
/// Capped at [`LIST_LIMIT`] rows.
///
/// # Errors
/// Returns an error if the outcomes reader is not wired, the range is invalid,
/// or the query fails.
pub async fn list_outcomes(
    State(state): State<AppState>,
    Query(q): Query<ListOutcomesQuery>,
) -> ApiResult<Json<Vec<OutcomeRecord>>> {
    /// Hard cap on rows returned by the list endpoint.
    const LIST_LIMIT: i64 = 500;

    let reader = state
        .outcomes_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcomes reader not wired".into()))?;

    let range = OutcomeRange::from_shorthand(q.range.as_deref().unwrap_or("30d"))
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

    let records = reader
        .list(q.kind.as_deref(), range, LIST_LIMIT)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcomes list: {e}")))?;

    Ok(Json(records))
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/summary
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct SummaryQuery {
    /// `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`.
    pub range: Option<String>,
}

/// Aggregated ROI summary — one bucket per outcome kind.  Backs the four
/// summary cards in the admin-ui Outcomes pane.
///
/// # Errors
/// Returns an error if the outcomes reader is not wired, the range is invalid, or the query fails.
pub async fn outcomes_summary(
    State(state): State<AppState>,
    Query(q): Query<SummaryQuery>,
) -> ApiResult<Json<OutcomesSummaryResponse>> {
    let reader = state
        .outcomes_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcomes reader not wired".into()))?;

    let range_str = q.range.as_deref().unwrap_or("30d");
    let range = OutcomeRange::from_shorthand(range_str)
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

    let summary = reader
        .summary(range)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcomes summary: {e}")))?;

    Ok(Json(OutcomesSummaryResponse {
        range: range_str.to_owned(),
        summary,
    }))
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/timeseries
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct TimeseriesQuery {
    /// `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`.
    pub range: Option<String>,
    /// Optional kind filter: `"revenue_usd"` | `"hours_saved"` | …
    pub kind: Option<String>,
}

/// Daily time-series breakdown — used by the bar chart in the Outcomes pane.
///
/// # Errors
/// Returns an error if the outcomes reader is not wired, the range is invalid, or the query fails.
pub async fn outcomes_timeseries(
    State(state): State<AppState>,
    Query(q): Query<TimeseriesQuery>,
) -> ApiResult<Json<OutcomesTimeseriesResponse>> {
    let reader = state
        .outcomes_reader
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("outcomes reader not wired".into()))?;

    let range_str = q.range.as_deref().unwrap_or("30d");
    let range = OutcomeRange::from_shorthand(range_str)
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

    let days = reader
        .timeseries(q.kind.as_deref(), range)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("outcomes timeseries: {e}")))?;

    Ok(Json(OutcomesTimeseriesResponse {
        range: range_str.to_owned(),
        days,
    }))
}
