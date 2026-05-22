//! Session CRUD + message append + SSE-streamed agent run.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{KeepAlive, Sse};
use axum::Json;
use chrono::Utc;
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use xiaoguai_agent::{ReactAgent, StopReason};
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_types::{Session, SessionId, SessionStatus, TenantId, UserId};

use crate::auth::Claims;
use crate::convert::{domain_to_llm, llm_to_domain};
use crate::error::{ApiError, ApiResult};
use crate::sse::event_to_sse;
use crate::state::AppState;

const DEFAULT_LIST_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, Default)]
pub struct CreateSessionRequest {
    /// In auth-required mode, claims `sub`/`tenant_id` win. In unauthed
    /// mode (v0.5.5 default for dev/test) the body must supply them.
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub tenant_id: String,
    pub model: String,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub title: Option<String>,
    pub model: String,
    pub status: SessionStatus,
}

impl From<Session> for SessionResponse {
    fn from(s: Session) -> Self {
        Self {
            id: s.id.to_string(),
            tenant_id: s.tenant_id.to_string(),
            user_id: s.user_id.to_string(),
            title: s.title,
            model: s.model,
            status: s.status,
        }
    }
}

pub async fn create_session(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    Json(req): Json<CreateSessionRequest>,
) -> ApiResult<(StatusCode, Json<SessionResponse>)> {
    // Claims override body identity when present (auth-required mode).
    let (user_id, tenant_id) = match claims.as_ref() {
        Some(Extension(c)) => (c.sub.clone(), c.tenant_id.clone()),
        None => (req.user_id.clone(), req.tenant_id.clone()),
    };
    if user_id.is_empty() || tenant_id.is_empty() || req.model.is_empty() {
        return Err(ApiError::BadRequest(
            "user_id, tenant_id, and model are required".into(),
        ));
    }
    let now = Utc::now();
    let session = Session {
        id: SessionId::new(),
        tenant_id: TenantId::from(tenant_id),
        user_id: UserId::from(user_id),
        title: req.title,
        created_at: now,
        updated_at: now,
        model: req.model,
        status: SessionStatus::Active,
    };
    state.sessions.create(&session).await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

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

#[derive(Debug, Deserialize)]
pub struct ListMessagesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

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
}

/// `POST /v1/sessions/:id/messages` — streams `AgentEvent`s as SSE.
///
/// Flow:
///   1. Persist the user message (one DB write).
///   2. Load full session history.
///   3. Launch `ReactAgent::run_stream`; thread a fresh `CancellationToken`
///      registered in `state.cancels`.
///   4. Stream each `AgentEvent` as one SSE event to the client.
///   5. A separately-spawned finalize task awaits the agent join handle and
///      persists any new messages, then drops the cancel registry entry.
pub async fn send_message(
    State(state): State<AppState>,
    Path(session_id_str): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, axum::Error>>>> {
    if req.content.trim().is_empty() {
        return Err(ApiError::BadRequest("content must be non-empty".into()));
    }
    let session = state
        .sessions
        .find_by_id(&session_id_str)
        .await?
        .ok_or(ApiError::NotFound)?;
    if !matches!(session.status, SessionStatus::Active) {
        return Err(ApiError::Conflict("session is not active".into()));
    }
    let session_id = SessionId::from(session_id_str.clone());

    // 1. Persist user message.
    let user_domain = persist_user_message(&state, &session_id, &req.content).await?;

    // 2. Load history (oldest-first) and append the just-written user msg.
    let history = load_llm_history(&state, &session_id_str).await?;
    let initial_count = history.len();
    let mut messages = history;
    messages.push(domain_to_llm(&user_domain));

    // 3. Build the agent and register a cancel token.
    let model = req.model.unwrap_or(session.model);
    let mut cfg = state.agent_defaults.clone();
    cfg.model = model;
    let agent = ReactAgent::new(state.backend.clone(), (*state.toolbox).clone(), cfg);
    let cancel = state.cancels.register(&session_id_str);

    // 4. Launch the loop. `events` will close naturally when the agent
    //    finishes; `join` resolves once the task body returns.
    let (join, events) = agent.run_stream(messages, cancel);

    // 5. Spawn the finalisation task — it runs concurrently with the SSE
    //    stream and persists anything the loop produced once the join
    //    handle resolves.
    spawn_finalize_task(
        state.clone(),
        session_id_str.clone(),
        session_id,
        join,
        initial_count + 1,
    );

    let sse_stream = events.map(|ev| Ok::<_, axum::Error>(event_to_sse(&ev)));
    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn spawn_finalize_task(
    state: AppState,
    session_id_str: String,
    session_id: SessionId,
    join: tokio::task::JoinHandle<Result<xiaoguai_agent::AgentOutcome, xiaoguai_agent::AgentError>>,
    initial_count: usize,
) {
    tokio::spawn(async move {
        match join.await {
            Ok(Ok(outcome)) => {
                if let Err(err) =
                    persist_loop_output(&state, &session_id, &outcome.messages, initial_count).await
                {
                    tracing::error!(?err, "failed to persist agent output");
                }
                tracing::info!(
                    stop_reason = ?outcome.stop_reason,
                    iterations = outcome.iterations,
                    "agent run finished"
                );
                let _: StopReason = outcome.stop_reason;
            }
            Ok(Err(err)) => tracing::error!(?err, "agent run errored"),
            Err(err) => tracing::error!(?err, "agent task panicked"),
        }
        state.cancels.drop_entry(&session_id_str);
        if let Err(err) = state.sessions.touch(&session_id_str).await {
            tracing::warn!(?err, "touch session failed");
        }
    });
}

#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub cancelled: bool,
}

pub async fn cancel_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<CancelResponse>> {
    let cancelled = state.cancels.cancel(&session_id);
    Ok(Json(CancelResponse { cancelled }))
}

// -- helpers ------------------------------------------------------------

async fn persist_user_message(
    state: &AppState,
    session_id: &SessionId,
    text: &str,
) -> ApiResult<xiaoguai_types::Message> {
    let llm = LlmMessage::user(text);
    let domain = llm_to_domain(session_id, &llm);
    state.messages.append(&domain).await?;
    Ok(domain)
}

async fn load_llm_history(state: &AppState, session_id: &str) -> ApiResult<Vec<LlmMessage>> {
    // We deliberately load *all* messages here; the agent loop applies its
    // own sliding window before each model call. Pagination at this layer
    // is a v0.5.5.1 concern.
    let domain = state
        .messages
        .list_by_session(session_id, i64::from(i32::MAX), 0)
        .await?;
    Ok(domain.iter().map(domain_to_llm).collect())
}

async fn persist_loop_output(
    state: &AppState,
    session_id: &SessionId,
    messages: &[LlmMessage],
    initial_count: usize,
) -> ApiResult<()> {
    let new_msgs: Vec<_> = messages
        .iter()
        .skip(initial_count)
        .map(|m| llm_to_domain(session_id, m))
        .collect();
    let messages_repo = Arc::clone(&state.messages);
    for m in new_msgs {
        messages_repo.append(&m).await?;
    }
    Ok(())
}
