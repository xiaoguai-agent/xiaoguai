//! `/v1/loops` — REST surface for /loop session-scoped recurring agent
//! turns (DEC-039 / LLD-LOOP-001 §6). Thin adapters over
//! [`crate::loops::LoopController`]; owner-auth like everything else
//! under `/v1`. All endpoints return 503 when the controller is unwired.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use xiaoguai_storage::repositories::LoopRow;

use crate::auth::Claims;
use crate::error::{ApiError, ApiResult};
use crate::loops::{CancelLoopError, CreateLoopError, CreateLoopParams, LoopController};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateLoopRequest {
    pub session_id: String,
    pub prompt: String,
    #[serde(default)]
    pub interval_secs: Option<u32>,
    #[serde(default)]
    pub max_ticks: Option<u32>,
    #[serde(default)]
    pub ttl_secs: Option<u32>,
    /// L3 Part B — let the agent pace the loop via `loop_next_tick`.
    #[serde(default)]
    pub dynamic_pacing: bool,
    #[serde(default)]
    pub min_interval_secs: Option<u32>,
    #[serde(default)]
    pub max_interval_secs: Option<u32>,
    /// L3 Part C — token budget; `0` = unlimited, omitted = 500k default.
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct LoopResponse {
    pub id: Uuid,
    pub session_id: String,
    pub prompt: String,
    pub pacing_kind: &'static str,
    pub interval_secs: u32,
    pub min_interval_secs: u32,
    pub max_interval_secs: u32,
    pub max_ticks: u32,
    pub ttl_secs: u32,
    pub max_total_tokens: u64,
    pub status: &'static str,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub next_tick_at: DateTime<Utc>,
    pub ticks_run: u32,
    pub consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl From<LoopRow> for LoopResponse {
    fn from(r: LoopRow) -> Self {
        Self {
            id: r.id,
            session_id: r.session_id,
            prompt: r.prompt,
            pacing_kind: r.pacing_kind.as_str(),
            interval_secs: r.interval_secs,
            min_interval_secs: r.min_interval_secs,
            max_interval_secs: r.max_interval_secs,
            max_ticks: r.max_ticks,
            ttl_secs: r.ttl_secs,
            max_total_tokens: r.max_total_tokens,
            status: r.status.as_str(),
            created_by: r.created_by,
            created_at: r.created_at,
            expires_at: r.expires_at,
            next_tick_at: r.next_tick_at,
            ticks_run: r.ticks_run,
            consecutive_failures: r.consecutive_failures,
            last_error: r.last_error,
        }
    }
}

fn controller(state: &AppState) -> ApiResult<Arc<LoopController>> {
    state
        .loops
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("loops are not wired on this server".into()))
}

/// `POST /v1/loops` — create + arm a loop on a session.
///
/// # Errors
/// 400 invalid budgets/prompt; 404 unknown session; 409 session archived
/// or the session already has a live loop; 503 unwired.
pub async fn create_loop(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Json(req): Json<CreateLoopRequest>,
) -> ApiResult<(StatusCode, Json<LoopResponse>)> {
    let ctrl = controller(&state)?;
    let row = ctrl
        .create(CreateLoopParams {
            session_id: req.session_id,
            prompt: req.prompt,
            interval_secs: req.interval_secs,
            max_ticks: req.max_ticks,
            ttl_secs: req.ttl_secs,
            dynamic_pacing: req.dynamic_pacing,
            min_interval_secs: req.min_interval_secs,
            max_interval_secs: req.max_interval_secs,
            max_total_tokens: req.max_total_tokens,
            created_by: claims.as_ref().map(|Extension(c)| c.sub.clone()),
        })
        .await
        .map_err(create_error_to_api)?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// `GET /v1/loops` — all loops, newest first (terminal rows included).
///
/// # Errors
/// 503 unwired; 500 on store failure.
pub async fn list_loops(State(state): State<AppState>) -> ApiResult<Json<Vec<LoopResponse>>> {
    let ctrl = controller(&state)?;
    let rows = ctrl.list().await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// `GET /v1/loops/:id`.
///
/// # Errors
/// 400 malformed id; 404 unknown; 503 unwired.
pub async fn get_loop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<LoopResponse>> {
    let ctrl = controller(&state)?;
    let id = parse_loop_id(&id)?;
    let row = ctrl.get(id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(row.into()))
}

/// `DELETE /v1/loops/:id` — cancel a live loop.
///
/// # Errors
/// 400 malformed id; 404 unknown; 409 already terminal; 503 unwired.
pub async fn cancel_loop(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Path(id): Path<String>,
) -> ApiResult<Json<LoopResponse>> {
    let ctrl = controller(&state)?;
    let id = parse_loop_id(&id)?;
    let cancelled_by = claims
        .as_ref()
        .map_or_else(|| "owner".to_string(), |Extension(c)| c.sub.clone());
    let row = ctrl
        .cancel(id, &cancelled_by)
        .await
        .map_err(cancel_error_to_api)?;
    Ok(Json(row.into()))
}

/// `POST /v1/loops/:id/resume` — resume a paused loop (undo `loop_pause`).
///
/// # Errors
/// 400 malformed id; 404 unknown; 409 not paused; 503 unwired.
pub async fn resume_loop(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Path(id): Path<String>,
) -> ApiResult<Json<LoopResponse>> {
    let ctrl = controller(&state)?;
    let id = parse_loop_id(&id)?;
    let resumed_by = claims
        .as_ref()
        .map_or_else(|| "owner".to_string(), |Extension(c)| c.sub.clone());
    let row = ctrl
        .resume(id, &resumed_by)
        .await
        .map_err(resume_error_to_api)?;
    Ok(Json(row.into()))
}

fn parse_loop_id(raw: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(raw).map_err(|_| ApiError::BadRequest(format!("malformed loop id: {raw}")))
}

fn resume_error_to_api(err: crate::loops::ResumeLoopError) -> ApiError {
    use crate::loops::ResumeLoopError;
    match err {
        ResumeLoopError::NotFound => ApiError::NotFound,
        ResumeLoopError::NotPaused(status) => {
            ApiError::Conflict(format!("loop is not paused (status: {status})"))
        }
        ResumeLoopError::Repo(e) => ApiError::Storage(e),
    }
}

fn create_error_to_api(err: CreateLoopError) -> ApiError {
    match err {
        CreateLoopError::SessionNotFound => ApiError::NotFound,
        CreateLoopError::SessionNotActive => ApiError::Conflict("session is not active".into()),
        CreateLoopError::AlreadyExists { existing } => ApiError::Conflict(format!(
            "session already has a live loop ({existing}) — cancel it first \
             (DELETE /v1/loops/{existing})"
        )),
        CreateLoopError::InvalidArgument(msg) => ApiError::BadRequest(msg),
        CreateLoopError::Repo(e) => ApiError::Storage(e),
    }
}

fn cancel_error_to_api(err: CancelLoopError) -> ApiError {
    match err {
        CancelLoopError::NotFound => ApiError::NotFound,
        CancelLoopError::AlreadyTerminal(status) => {
            ApiError::Conflict(format!("loop is already terminal ({status})"))
        }
        CancelLoopError::Repo(e) => ApiError::Storage(e),
    }
}
