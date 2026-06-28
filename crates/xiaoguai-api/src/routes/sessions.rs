//! Session CRUD + message append + SSE-streamed agent run.

use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{KeepAlive, Sse};
use axum::Json;
use chrono::Utc;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use xiaoguai_types::{Session, SessionId, SessionStatus, UserId};

use crate::auth::Claims;
use crate::error::{ApiError, ApiResult};
use crate::sessions_ext::SessionForkError;
use crate::sse::event_to_sse_seq;
use crate::state::AppState;
use crate::turn::{run_turn, TurnError, TurnInput, TurnMode};

const DEFAULT_LIST_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, Default)]
pub struct CreateSessionRequest {
    /// In auth-required mode, claims `sub` wins. In unauthed mode (v0.5.5
    /// default for dev/test) the body must supply it.
    #[serde(default)]
    pub user_id: String,
    /// Empty (or omitted) lets the LLM router pick its default model — the
    /// primary (lowest `fallback_order`) provider's first model.
    #[serde(default)]
    pub model: String,
    pub title: Option<String>,
    /// Feature ⑤ — optional per-session coding workspace root (absolute
    /// server path). Omitted/`None` lets the session fall back to the global
    /// default (`XIAOGUAI_CODING_WORKSPACE`).
    pub working_dir: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_id: String,
    pub title: Option<String>,
    pub model: String,
    pub status: SessionStatus,
    /// v1.1.2 — when the row was created via `POST /v1/sessions/:id/fork`,
    /// the parent session's id. `None` for top-level sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// v1.1.2 — companion to `parent_session_id`: the last message of
    /// the parent that was copied into this session at fork time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_message_id: Option<String>,
    /// Feature ⑤ — per-session coding workspace root (absolute server path).
    /// Omitted from the response when unset (the session uses the global
    /// default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
}

impl From<Session> for SessionResponse {
    fn from(s: Session) -> Self {
        Self {
            id: s.id.to_string(),
            user_id: s.user_id.to_string(),
            title: s.title,
            model: s.model,
            status: s.status,
            parent_session_id: s.parent_session_id.map(|id| id.to_string()),
            forked_from_message_id: s.forked_from_message_id.map(|id| id.to_string()),
            working_dir: s.working_dir,
        }
    }
}

/// # Errors
/// Returns an error if required fields are missing, the session store is not wired, or the create fails.
pub async fn create_session(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Json(req): Json<CreateSessionRequest>,
) -> ApiResult<(StatusCode, Json<SessionResponse>)> {
    // Claims override body identity when present (owner-auth mode).
    let user_id = match claims.as_ref() {
        Some(Extension(c)) => c.sub.clone(),
        None => req.user_id.clone(),
    };
    // `model` may be empty — the router substitutes its default model at chat
    // time (the primary provider's first model). Only `user_id` is required.
    if user_id.is_empty() {
        return Err(ApiError::BadRequest("user_id is required".into()));
    }
    let now = Utc::now();
    let session = Session {
        id: SessionId::new(),
        user_id: UserId::from(user_id),
        title: req.title,
        created_at: now,
        updated_at: now,
        model: req.model,
        status: SessionStatus::Active,
        parent_session_id: None,
        forked_from_message_id: None,
        working_dir: req.working_dir,
    };
    state.sessions.create(&session).await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListSessionsQuery {
    pub user_id: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `GET /v1/sessions?user_id=...&limit=...&offset=...` — list a user's
/// sessions ordered by most-recently-updated first.
///
/// `user_id` is required: in dev mode it comes from the query string;
/// in authed mode it falls back to `Claims.sub` when missing.
///
/// # Errors
/// Returns an error if `user_id` is missing, the session store is not wired, or the query fails.
pub async fn list_sessions(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Query(q): Query<ListSessionsQuery>,
) -> ApiResult<Json<Vec<SessionResponse>>> {
    let user_id = q
        .user_id
        .or_else(|| claims.as_ref().map(|Extension(c)| c.sub.clone()))
        .ok_or_else(|| ApiError::BadRequest("user_id is required".into()))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = state.sessions.list_by_user(&user_id, limit, offset).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// # Errors
/// Returns an error if the session is not found or the session store fails.
pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<SessionResponse>> {
    let session = state
        .sessions
        .find_by_id(&session_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(session.into()))
}

/// Feature ⑤ — partial update of a session. Only the fields present in the
/// body are changed (PATCH semantics): omitting `working_dir` keeps the
/// stored value; sending `""` clears the per-session override so the session
/// falls back to the global coding workspace default.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateSessionRequest {
    pub title: Option<String>,
    pub working_dir: Option<String>,
}

/// `PATCH /v1/sessions/:id` — update a session's mutable metadata
/// (`title`, `working_dir`). 404 when the session does not exist.
///
/// # Errors
/// Returns an error if the session is not found or the session store fails.
pub async fn update_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<UpdateSessionRequest>,
) -> ApiResult<Json<SessionResponse>> {
    // Existence check up front so a missing session is a clean 404 rather
    // than a silent no-op UPDATE (the repo also guards, but checking here
    // keeps the contract explicit and lets us return the refreshed row).
    state
        .sessions
        .find_by_id(&session_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state
        .sessions
        .update(&session_id, req.title, req.working_dir)
        .await
        .map_err(|e| match e {
            xiaoguai_storage::repositories::RepoError::NotFound => ApiError::NotFound,
            other => ApiError::Storage(other),
        })?;
    let updated = state
        .sessions
        .find_by_id(&session_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(updated.into()))
}

#[derive(Debug, Deserialize)]
pub struct ListMessagesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// # Errors
/// Returns an error if the session is not found or the message store fails.
pub async fn list_messages(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(q): Query<ListMessagesQuery>,
) -> ApiResult<Json<Vec<xiaoguai_types::Message>>> {
    // Existence check so we return 404 instead of an empty list.
    let _session = state
        .sessions
        .find_by_id(&session_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let msgs = state
        .messages
        .list_by_session(&session_id, limit, offset)
        .await?;
    Ok(Json(msgs))
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    /// Optional model override; falls back to the session's model.
    pub model: Option<String>,
    /// T5: `"consult"` makes the turn read-only; omitted/`"execute"` is the
    /// normal HotL-gated mode. Per-turn flag — no session state.
    pub mode: Option<TurnMode>,
}

/// `POST /v1/sessions/:id/messages` — streams `AgentEvent`s as SSE.
///
/// The turn pipeline itself (persist → history → identity → enforcer →
/// `run_streamed` → finalize) lives in [`crate::turn::run_turn`], shared
/// with the /loop controller. This handler is the SSE adapter: map
/// [`TurnError`]s onto HTTP statuses and stamp the event stream.
///
/// # Errors
/// Returns an error if the session is not found, the message is invalid,
/// a turn is already in flight for the session (409), or the agent fails
/// to start.
pub async fn send_message(
    State(state): State<AppState>,
    Path(session_id_str): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, axum::Error>>>> {
    // `completion` is dropped — the SSE client learns the outcome from the
    // event stream itself; the finalize task's send is best-effort.
    let handle = run_turn(
        &state,
        TurnInput {
            session_id: session_id_str,
            content: req.content,
            model_override: req.model,
            mode: req.mode.unwrap_or_default(),
            loop_id: None,
            loop_dynamic_pacing: false,
        },
    )
    .await
    .map_err(turn_error_to_api)?;
    let events = handle.events;

    // Stamp each event with a per-stream monotonic id (`id:` field). The
    // client echoes the last seen id as `Last-Event-ID` on reconnect and
    // uses it to drop a superseded turn (F5 SSE reconnect de-dup). The
    // sequence restarts per response — this stream carries no cross-request
    // resume state.
    let sse_stream = events
        .enumerate()
        .map(|(i, ev)| Ok::<_, axum::Error>(event_to_sse_seq(&ev, i as u64 + 1)));
    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Map a refused turn onto the route's HTTP contract. Messages are part of
/// the public API surface — keep them stable (clients switch on `code`,
/// humans read the message).
fn turn_error_to_api(err: TurnError) -> ApiError {
    match err {
        TurnError::EmptyContent => ApiError::BadRequest("content must be non-empty".into()),
        TurnError::SessionNotFound => ApiError::NotFound,
        TurnError::SessionNotActive => ApiError::Conflict("session is not active".into()),
        TurnError::TurnInFlight => {
            ApiError::Conflict("a turn is already in flight for this session".into())
        }
        TurnError::HotlDenied(reason) => {
            ApiError::ServiceUnavailable(format!("LLM call denied by HOTL policy: {reason}"))
        }
        TurnError::HotlUnavailable => {
            ApiError::ServiceUnavailable("LLM call denied: HOTL enforcer unavailable".into())
        }
        TurnError::Repo(e) => ApiError::Storage(e),
    }
}

#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// # Errors
/// Returns an error if the session is not found or cannot be cancelled.
pub async fn cancel_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<CancelResponse>> {
    let cancelled = state.cancels.cancel(&session_id);
    Ok(Json(CancelResponse { cancelled }))
}

// -- v1.1.2: conversation fork -----------------------------------------

#[derive(Debug, Deserialize)]
pub struct ForkSessionRequest {
    pub from_message_id: String,
    pub title: Option<String>,
}

/// `POST /v1/sessions/:id/fork` — branch this session at
/// `from_message_id`. The new session starts with a copy of every
/// message from the parent up to and including the cutoff; the
/// caller can then `POST .../messages` against the new id to take
/// the conversation in a different direction.
///
/// # Errors
/// Returns an error if the parent session is not found, the message ID is invalid, or the fork fails.
pub async fn fork_session(
    State(state): State<AppState>,
    Path(session_id_str): Path<String>,
    Json(req): Json<ForkSessionRequest>,
) -> ApiResult<(StatusCode, Json<SessionResponse>)> {
    let forker = state
        .session_forker
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("fork not wired".into()))?
        .clone();

    if req.from_message_id.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "from_message_id must be non-empty".into(),
        ));
    }

    // Verify the parent session exists before forking — a missing parent
    // is a 404, not a freshly-minted child.
    state
        .sessions
        .find_by_id(&session_id_str)
        .await?
        .ok_or(ApiError::NotFound)?;

    let new_session = forker
        .fork(&session_id_str, &req.from_message_id, req.title)
        .await
        .map_err(fork_error_to_api)?;
    Ok((StatusCode::CREATED, Json(new_session.into())))
}

fn fork_error_to_api(err: SessionForkError) -> ApiError {
    match err {
        SessionForkError::ParentNotFound | SessionForkError::MessageNotFound => ApiError::NotFound,
        SessionForkError::ParentNotForkable(s) => ApiError::Conflict(s),
        SessionForkError::InvalidArgument(s) => ApiError::BadRequest(s),
        SessionForkError::Repository(s) => {
            tracing::error!(err = %s, "session fork repository error");
            ApiError::Internal(anyhow::anyhow!("session fork: {s}"))
        }
    }
}
