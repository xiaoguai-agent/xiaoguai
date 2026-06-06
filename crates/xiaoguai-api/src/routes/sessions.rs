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
use crate::sse::event_to_sse_seq;
use crate::state::AppState;

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
    let mut messages = load_llm_history(&state, &session_id_str).await?;
    messages.push(domain_to_llm(&user_domain));

    // 2b. Identity memory (DEC-036, P1): prepend the owner's persistent `USER.md`
    //     profile as a leading System message so every session knows who it is
    //     working for. Loaded per-request (picks up edits without a restart);
    //     absent/blank file → no-op. Not persisted into the session history.
    //     The finalize task persists `outcome.new_messages` (anchored on the
    //     inbound prompt), so no prefix/skip arithmetic is needed here.
    if let Some(identity) = crate::identity::load_identity() {
        messages.insert(0, LlmMessage::system(identity));
    }

    // 3. Build the runtime context and register a cancel token.
    //    v0.12.0: every call site builds via RuntimeContext.
    let model = req.model.unwrap_or(session.model);
    let ctx = RuntimeContext::new(
        state.backend.clone(),
        state.toolbox.clone(),
        state.agent_defaults.clone(),
    )
    .with_model(model.clone());
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
    spawn_finalize_task(FinalizeCtx {
        state: state.clone(),
        session_id_str: session_id_str.clone(),
        session_id,
        actor: session.user_id.to_string(),
        model,
        join,
    });

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

/// Inputs to the detached finalisation task. Bundled into one struct so the
/// spawn site stays readable as the audit/identity wiring grows.
struct FinalizeCtx {
    state: AppState,
    session_id_str: String,
    session_id: SessionId,
    /// Audit actor — the session owner (`session.user_id`).
    actor: String,
    /// Resolved model for this turn (request override or session default).
    model: String,
    join: tokio::task::JoinHandle<Result<RuntimeOutcome, xiaoguai_runtime::RuntimeError>>,
}

fn spawn_finalize_task(ctx: FinalizeCtx) {
    let FinalizeCtx {
        state,
        session_id_str,
        session_id,
        actor,
        model,
        join,
    } = ctx;
    tokio::spawn(async move {
        // Audit-completeness: the SSE chat path runs the agent via
        // `run_streamed` + this detached finaliser, so it never goes through
        // the runtime's audit sink. Emit one HMAC-chained `agent.run` entry
        // per run — completed, errored, or panicked — (same route-level
        // pattern as `hotl.decision`). Best-effort: an audit failure here
        // must NOT affect the already-finished run. Details are content-free
        // by design: counts and enum tags only, never message text or error
        // strings (provider errors can embed response fragments).
        match join.await {
            Ok(Ok(outcome)) => {
                let persist_failed = match persist_loop_output(&state, &session_id, &outcome).await
                {
                    Ok(_) => false,
                    Err(err) => {
                        tracing::error!(?err, "failed to persist agent output");
                        true
                    }
                };
                append_agent_run_audit(
                    &state,
                    &actor,
                    &session_id_str,
                    agent_run_details(&model, &outcome, persist_failed),
                )
                .await;
                tracing::info!(
                    stop_reason = ?outcome.stop_reason,
                    iterations = outcome.iterations,
                    "agent run finished"
                );
                let _: StopReason = outcome.stop_reason;
            }
            Ok(Err(err)) => {
                tracing::error!(?err, "agent run errored");
                append_agent_run_audit(
                    &state,
                    &actor,
                    &session_id_str,
                    serde_json::json!({ "model": model, "outcome": "error" }),
                )
                .await;
            }
            Err(err) => {
                tracing::error!(?err, "agent task panicked");
                append_agent_run_audit(
                    &state,
                    &actor,
                    &session_id_str,
                    serde_json::json!({ "model": model, "outcome": "panic" }),
                )
                .await;
            }
        }
        state.cancels.drop_entry(&session_id_str);
        if let Err(err) = state.sessions.touch(&session_id_str).await {
            tracing::warn!(?err, "touch session failed");
        }
    });
}

/// Best-effort append of an `agent.run` entry to the HMAC chain. A missing
/// sink or an append failure is logged and never affects the run.
async fn append_agent_run_audit(
    state: &AppState,
    actor: &str,
    session_id: &str,
    details: serde_json::Value,
) {
    let Some(sink) = &state.hotl_audit else {
        return;
    };
    if let Err(err) = sink
        .append(build_agent_run_audit(actor, session_id, details))
        .await
    {
        tracing::warn!(%err, "agent.run audit append failed");
    }
}

/// Build the `agent.run` audit entry shell (timestamp stamped at call time;
/// everything else is deterministic and unit-tested).
fn build_agent_run_audit(
    actor: &str,
    session_id: &str,
    details: serde_json::Value,
) -> xiaoguai_audit::AuditEntry {
    xiaoguai_audit::AuditEntry {
        ts: Utc::now(),
        tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
        actor: actor.to_string(),
        action: "agent.run".into(),
        resource: Some(format!("session:{session_id}")),
        details,
    }
}

/// Details payload for a completed run. `messages_produced` is derived from
/// `outcome.new_messages` — the runtime's authoritative "produced this run"
/// slice (anchored on the inbound prompt, robust to history-window trimming)
/// — minus the inbound user message it includes. `persist_failed` lets an
/// auditor reconcile the chain against the `messages` table.
fn agent_run_details(
    model: &str,
    outcome: &RuntimeOutcome,
    persist_failed: bool,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "stop_reason": format!("{:?}", outcome.stop_reason),
        "iterations": outcome.iterations,
        "messages_produced": outcome.new_messages.len().saturating_sub(1),
        "persist_failed": persist_failed,
    })
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

/// Persist the messages this run produced, selected by [`messages_to_persist`].
///
/// Audit-review H1: this used to `skip(prefix_len)` over `outcome.messages`,
/// but the agent's history-window `slide()` trims that vec BEFORE the run, so
/// any session longer than the window made the skip swallow the entire run —
/// the assistant reply streamed to the client was silently never persisted.
/// `outcome.new_messages` (anchored on the inbound prompt) is the runtime's
/// authoritative slice and is what the IM gateway already uses.
async fn persist_loop_output(
    state: &AppState,
    session_id: &SessionId,
    outcome: &RuntimeOutcome,
) -> ApiResult<usize> {
    let to_persist = messages_to_persist(outcome);
    let messages_repo = Arc::clone(&state.messages);
    for m in &to_persist {
        messages_repo.append(&llm_to_domain(session_id, m)).await?;
    }
    Ok(to_persist.len())
}

/// Select which of the run's messages to persist (pure, unit-tested).
///
/// `outcome.new_messages` is `[inbound user msg, ...turns produced this run]`;
/// the inbound message was already persisted by `persist_user_message`, so it
/// is skipped. Defensive fallback (mirrors the IM gateway's v0.7.4 behaviour):
/// when the slide window dropped the inbound prompt — `new_messages` empty —
/// persist at least the reply text so the answer the client already streamed
/// is not lost.
fn messages_to_persist(outcome: &RuntimeOutcome) -> Vec<LlmMessage> {
    if outcome.new_messages.is_empty() {
        if outcome.reply_text.is_empty() {
            Vec::new()
        } else {
            vec![LlmMessage::assistant(&outcome.reply_text)]
        }
    } else {
        outcome.new_messages[1..].to_vec()
    }
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

#[cfg(test)]
mod audit_tests {
    use super::*;
    use xiaoguai_agent::StopReason;

    fn outcome(
        messages: Vec<LlmMessage>,
        new_messages: Vec<LlmMessage>,
        reply_text: &str,
        stop: StopReason,
        iterations: u32,
    ) -> RuntimeOutcome {
        RuntimeOutcome {
            stop_reason: stop,
            iterations,
            messages,
            new_messages,
            reply_text: reply_text.to_string(),
        }
    }

    #[test]
    fn agent_run_audit_carries_run_metadata() {
        let o = outcome(
            vec![LlmMessage::user("q"), LlmMessage::assistant("a")],
            vec![LlmMessage::user("q"), LlmMessage::assistant("a")],
            "a",
            StopReason::Completed,
            3,
        );
        let entry = build_agent_run_audit("owner", "sess-1", agent_run_details("gpt-x", &o, false));

        assert_eq!(entry.action, "agent.run");
        assert_eq!(entry.actor, "owner");
        assert_eq!(entry.resource.as_deref(), Some("session:sess-1"));
        assert_eq!(entry.tenant_id, xiaoguai_audit::OWNER_TENANT_ID);
        assert_eq!(entry.details["model"], "gpt-x");
        assert_eq!(entry.details["iterations"], 3);
        // new_messages = [inbound, assistant] → 1 produced.
        assert_eq!(entry.details["messages_produced"], 1);
        assert_eq!(entry.details["stop_reason"], "Completed");
        assert_eq!(entry.details["persist_failed"], false);
    }

    #[test]
    fn agent_run_audit_count_survives_history_window_trimming() {
        // Audit-review H1 regression: a long session gets trimmed by the
        // agent's slide() BEFORE the run, so `outcome.messages` is shorter
        // than the submitted history. The old `messages.len() - prefix_len`
        // arithmetic reported 0 here; `new_messages` stays correct.
        let trimmed: Vec<LlmMessage> = (0..32).map(|i| LlmMessage::user(i.to_string())).collect();
        let mut messages = trimmed;
        messages.push(LlmMessage::user("fresh prompt"));
        messages.push(LlmMessage::assistant("fresh answer"));
        let o = outcome(
            messages,
            vec![
                LlmMessage::user("fresh prompt"),
                LlmMessage::assistant("fresh answer"),
            ],
            "fresh answer",
            StopReason::Completed,
            1,
        );
        let details = agent_run_details("m", &o, false);
        assert_eq!(details["messages_produced"], 1);
    }

    #[test]
    fn agent_run_audit_messages_produced_saturates_when_empty() {
        let o = outcome(Vec::new(), Vec::new(), "", StopReason::MaxIterations, 10);
        let details = agent_run_details("m", &o, true);
        assert_eq!(details["messages_produced"], 0);
        assert_eq!(details["stop_reason"], "MaxIterations");
        assert_eq!(details["persist_failed"], true);
    }

    #[test]
    fn messages_to_persist_skips_already_persisted_inbound() {
        let o = outcome(
            Vec::new(),
            vec![
                LlmMessage::user("q"),
                LlmMessage::assistant("step"),
                LlmMessage::assistant("done"),
            ],
            "done",
            StopReason::Completed,
            2,
        );
        let out = messages_to_persist(&o);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "step");
        assert_eq!(out[1].content, "done");
    }

    #[test]
    fn messages_to_persist_falls_back_to_reply_text_when_inbound_trimmed() {
        // Extreme case: the run itself outgrew the window and the inbound
        // prompt was trimmed → new_messages is empty. The streamed answer
        // must still be persisted (v0.7.4 fallback, same as the IM gateway).
        let o = outcome(
            Vec::new(),
            Vec::new(),
            "the answer",
            StopReason::Completed,
            1,
        );
        let out = messages_to_persist(&o);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "the answer");
    }

    #[test]
    fn messages_to_persist_empty_when_nothing_produced() {
        let o = outcome(Vec::new(), Vec::new(), "", StopReason::Cancelled, 0);
        assert!(messages_to_persist(&o).is_empty());
    }
}
