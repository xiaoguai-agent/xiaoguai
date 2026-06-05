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
use xiaoguai_agent::StopReason;
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_runtime::{run_streamed, RuntimeContext, RuntimeOutcome};
use xiaoguai_types::{Session, SessionId, SessionStatus, UserId};

use crate::auth::Claims;
use crate::convert::{domain_to_llm, llm_to_domain};
use crate::error::{ApiError, ApiResult};
use crate::sessions_ext::SessionForkError;
use crate::sse::event_to_sse;
use crate::state::AppState;

const DEFAULT_LIST_LIMIT: i64 = 100;

#[derive(Debug, Deserialize, Default)]
pub struct CreateSessionRequest {
    /// In auth-required mode, claims `sub` wins. In unauthed mode (v0.5.5
    /// default for dev/test) the body must supply it.
    #[serde(default)]
    pub user_id: String,
    pub model: String,
    pub title: Option<String>,
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
    if user_id.is_empty() || req.model.is_empty() {
        return Err(ApiError::BadRequest(
            "user_id and model are required".into(),
        ));
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
///
/// # Errors
/// Returns an error if the session is not found, the message is invalid, or the agent fails to start.
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

    // 2b. Identity memory (DEC-036, P1): prepend the owner's persistent `USER.md`
    //     profile as a leading System message so every session knows who it is
    //     working for. Loaded per-request (picks up edits without a restart);
    //     absent/blank file → no-op. Not persisted into the session history.
    //     The prepend shifts `outcome.messages` by one, so the finalize skip
    //     count must account for it (else the user message is re-persisted every
    //     turn — see `prefix_len` below).
    let mut prefix_len = initial_count + 1; // history + the user message
    if let Some(identity) = crate::identity::load_identity() {
        messages.insert(0, LlmMessage::system(identity));
        prefix_len += 1;
    }

    // 3. Build the runtime context and register a cancel token.
    //    v0.12.0: every call site builds via RuntimeContext.
    let model = req.model.unwrap_or(session.model);
    let ctx = RuntimeContext::new(
        state.backend.clone(),
        state.toolbox.clone(),
        state.agent_defaults.clone(),
    )
    .with_model(model);
    let cancel = state.cancels.register(&session_id_str);

    // 4. HOTL budget check — gated on the "llm_call" scope.
    //    Fail-closed: if the enforcer returns Deny, abort before spawning the
    //    agent loop. Escalate is logged and the call proceeds (async review).
    //    When `hotl_enforcer` is None (dev / tests without budget), skip.
    if let Some(enforcer) = &state.hotl_enforcer {
        match enforcer.check("llm_call", 1.0).await {
            Ok(crate::hotl::enforcer::HotlVerdict::Allow) => {}
            Ok(crate::hotl::enforcer::HotlVerdict::Escalate(reason)) => {
                tracing::warn!(%reason, "HOTL escalation triggered");
            }
            Ok(crate::hotl::enforcer::HotlVerdict::Deny(reason)) => {
                tracing::warn!(%reason, "HOTL denied LLM call");
                return Err(ApiError::ServiceUnavailable(format!(
                    "LLM call denied by HOTL policy: {reason}"
                )));
            }
            Err(e) => {
                // Enforcer itself errored — fail-closed.
                tracing::error!(?e, "HOTL enforcer error — denying LLM call (fail-closed)");
                return Err(ApiError::ServiceUnavailable(
                    "LLM call denied: HOTL enforcer unavailable".into(),
                ));
            }
        }
    }

    // 5. Launch the loop via the runtime. `events` closes naturally when
    //    the loop terminates; `join` resolves with the enriched outcome.
    let (join, events) = run_streamed(&ctx, messages, cancel);

    // 6. Spawn the finalisation task — it runs concurrently with the SSE
    //    stream and persists anything the loop produced once the join
    //    handle resolves.
    spawn_finalize_task(
        state.clone(),
        session_id_str.clone(),
        session_id,
        join,
        prefix_len,
    );

    let sse_stream = events.map(|ev| Ok::<_, axum::Error>(event_to_sse(&ev)));
    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn spawn_finalize_task(
    state: AppState,
    session_id_str: String,
    session_id: SessionId,
    join: tokio::task::JoinHandle<Result<RuntimeOutcome, xiaoguai_runtime::RuntimeError>>,
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

/// # Errors
/// Returns an error if the session is not found or cannot be cancelled.
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
