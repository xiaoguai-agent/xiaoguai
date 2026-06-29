//! The shared turn pipeline — persist → history → identity → enforcer →
//! `run_streamed` → finalize — extracted from the `send_message` SSE
//! handler so the /loop controller can run ticks through the exact same
//! code path (LLD-LOOP-001 §3, review C1/H1).
//!
//! [`run_turn`] acquires the per-session turn lock up front
//! ([`crate::state::CancelRegistry::try_begin_turn`]) and refuses to start
//! when a turn is already in flight: the route maps
//! [`TurnError::TurnInFlight`] to 409, the loop controller skips the tick.
//! The lock (and the turn's cancellation token — same registry entry)
//! is released by the detached finalize task once the run's output is
//! persisted, so a follow-up turn always sees the previous turn's messages.

use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;
use tokio_stream::wrappers::ReceiverStream;
use xiaoguai_agent::{AgentEvent, StopReason};
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_mcp::ActiveToolsSource;
use xiaoguai_runtime::{run_streamed, RuntimeContext, RuntimeError, RuntimeOutcome};
use xiaoguai_storage::repositories::RepoError;
use xiaoguai_types::{SessionId, SessionStatus};

use crate::convert::{domain_to_llm, llm_to_domain};
use crate::state::{AppState, TurnGuard};

/// Consult/execute split (T5, plan §2). `Execute` is today's behaviour;
/// `Consult` makes the turn read-only: the toolbox is narrowed to
/// `MutationHint::Read` tools (layer 1) and the `HotL` gate is wrapped in
/// `ConsultGate` (layer 2) so write tools cannot run even if named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnMode {
    #[default]
    Execute,
    Consult,
}

impl TurnMode {
    /// Wire/audit label — matches the serde rename (`consult` / `execute`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Consult => "consult",
        }
    }
}

/// One turn's inputs. `model_override` falls back to the session's model.
#[derive(Debug)]
pub struct TurnInput {
    pub session_id: String,
    pub content: String,
    pub model_override: Option<String>,
    /// Consult (read-only) vs execute. Ignored on loop turns
    /// (`loop_id.is_some()`) — loops always run execute (plan §2.4).
    pub mode: TurnMode,
    /// Set for loop ticks: stamps `initiator: "loop"` + `loop_id` into the
    /// turn's `agent.run` audit details (LLD-LOOP-001 §7, review M3 — an
    /// auditor must be able to tell loop-initiated turns from operator
    /// turns). `None` for operator turns; the details are unchanged.
    pub loop_id: Option<uuid::Uuid>,
    /// L3 Part B: when this is a dynamic-pacing loop tick, register the
    /// `loop_next_tick` tool so the agent can choose its own cadence.
    /// Ignored when `loop_id` is `None`.
    pub loop_dynamic_pacing: bool,
}

/// How a launched turn ended — reported by the finalize task over
/// [`TurnHandle::completion`]. Coarse by design: the /loop controller's
/// failure backoff (LLD-LOOP-001 §3) only needs success-or-not; details
/// live in the `agent.run` audit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCompletion {
    /// Run finished (any stop reason, including cancelled) and its output
    /// was persisted.
    Completed,
    /// The runtime returned an error (provider down, agent error).
    Errored,
    /// The agent task panicked.
    Panicked,
}

/// A successfully launched turn.
///
/// `events` is the live event stream (the SSE route consumes it; loop
/// ticks drop it — the agent's event channel sends fail fast on a dropped
/// receiver and the run keeps going). `completion` resolves when the
/// finalize task is done; dropping it is fine (the send is best-effort).
pub struct TurnHandle {
    pub events: ReceiverStream<AgentEvent>,
    pub completion: tokio::sync::oneshot::Receiver<TurnCompletion>,
    /// Set on loop turns (`TurnInput.loop_id == Some`): the cell the
    /// `loop_done` / `loop_pause` tools write. The controller reads it
    /// *after* `completion` resolves to decide the loop's next transition.
    /// `None` on operator turns.
    pub loop_intent: Option<crate::loop_tools::LoopToolSink>,
}

/// Why a turn refused to start. The route maps these onto HTTP statuses
/// (`turn_error_to_api` in `routes/sessions.rs`); the loop controller
/// switches on them directly.
#[derive(Debug, Error)]
pub enum TurnError {
    #[error("content must be non-empty")]
    EmptyContent,
    #[error("session not found")]
    SessionNotFound,
    #[error("session is not active")]
    SessionNotActive,
    /// A turn is already in flight for this session (per-session turn lock).
    #[error("a turn is already in flight for this session")]
    TurnInFlight,
    #[error("LLM call denied by HOTL policy: {0}")]
    HotlDenied(String),
    #[error("LLM call denied: HOTL enforcer unavailable")]
    HotlUnavailable,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// Run one agent turn on a session and return its live event stream.
///
/// Flow:
///   1. Acquire the per-session turn lock (refuse with [`TurnError::TurnInFlight`]).
///   2. Persist the user message (one DB write).
///   3. Load full session history; prepend identity memory (`USER.md`).
///   4. HOTL budget check — fail-closed.
///   5. Launch `run_streamed`; spawn a detached finalize task that persists
///      the run's output, appends the `agent.run` audit entry, then drops
///      the turn lock.
///
/// The caller may drop the returned stream (loop ticks do): the agent's
/// event channel sends fail fast on a dropped receiver and the run keeps
/// going to completion.
///
/// # Errors
/// Returns a [`TurnError`] when the turn cannot start; once the handle is
/// returned, run failures surface as [`TurnCompletion`] / events / audit
/// entries, not errors.
pub async fn run_turn(state: &AppState, input: TurnInput) -> Result<TurnHandle, TurnError> {
    if input.content.trim().is_empty() {
        return Err(TurnError::EmptyContent);
    }
    let session = state
        .sessions
        .find_by_id(&input.session_id)
        .await?
        .ok_or(TurnError::SessionNotFound)?;
    if !matches!(session.status, SessionStatus::Active) {
        return Err(TurnError::SessionNotActive);
    }

    // Per-session turn lock + this turn's cancellation token (one registry
    // entry — the guard releases both). Acquired before the first write so
    // a refused turn leaves no trace.
    let guard = state
        .cancels
        .try_begin_turn(&input.session_id)
        .ok_or(TurnError::TurnInFlight)?;
    let session_id = SessionId::from(input.session_id.clone());

    // 1. Persist user message.
    let user_domain = persist_user_message(state, &session_id, &input.content).await?;

    // 2. Load history (oldest-first) and append the just-written user msg.
    let mut messages = load_llm_history(state, &input.session_id).await?;
    messages.push(domain_to_llm(&user_domain));

    // 2b. Loop turns (L2): register the `loop_done` / `loop_pause` tools and
    //     nudge the agent to use them. The toolbox is built per-turn (the
    //     base toolbox plus the two built-ins sharing one intent sink) so
    //     these tools are invisible to ordinary operator turns. `None` on
    //     operator turns → the base toolbox, no extra cost. The system note
    //     is inserted here (before identity) so identity ends up the
    //     outermost System frame: [identity, loop_note, ...history].
    //     T5: loop turns ignore `mode` — they never set it, but guard anyway
    //     so a future caller cannot put a loop tick into consult mode by
    //     accident (plan §2.4: loops/scheduler stay execute).
    let mode = if input.loop_id.is_some() {
        TurnMode::Execute
    } else {
        input.mode
    };
    // Feature ⑤ (per-session working dir) — LIVE.
    //
    // `session.working_dir` is the absolute server path this session's coding
    // tools should use as their workspace root. At boot `state.toolbox` bakes
    // the *global* `XIAOGUAI_CODING_WORKSPACE` root into the coding tools; when
    // a session pins a *different* dir we rebuild the toolbox for THIS turn
    // with the coding tools rooted there (`state.coding_toolbox_factory`,
    // wired by `xiaoguai-core::run_serve` only when coding is enabled). All
    // gating/security is unchanged — the factory reproduces the same governed
    // surface, only the root differs. Resolved once here so the loop/consult
    // branches below layer onto the right base toolbox.
    //
    // Common path is byte-identical: no `working_dir`, no factory, or a dir
    // equal to the global root ⇒ `base` is just `state.toolbox` (no rebuild).
    let base = resolve_turn_base_toolbox(state, session.working_dir.as_deref()).await;
    // MCP live-tools merge — fold every ACTIVE MCP server's tools onto `base`
    // BEFORE the consult/loop layering below so an MCP tool's `MutationHint`
    // is honored exactly like a coding tool: a read MCP tool stays available
    // in consult mode (layer-1 subset), a write MCP tool is hidden/denied by
    // the `ConsultGate` (layer-2). Done per-turn (not at boot) so a server
    // installed at runtime is callable without a restart. Best-effort: any
    // failure logs and proceeds with the un-merged `base` — MCP availability
    // must never break a chat turn. Common path (no active servers) returns
    // `base` unchanged with no allocation.
    let base = merge_active_mcp_tools(
        base,
        state
            .mcp_supervisor
            .as_deref()
            .map(|s| s as &dyn ActiveToolsSource),
    )
    .await;
    let (toolbox, loop_intent) = if input.loop_id.is_some() {
        let (tb, sink) = crate::loop_tools::with_loop_tools(&base, input.loop_dynamic_pacing);
        messages.insert(0, LlmMessage::system(LOOP_TICK_SYSTEM_NOTE));
        (Arc::new(tb), Some(sink))
    } else if mode == TurnMode::Consult {
        // Layer 1 (plan §2.2): the model only sees read tools.
        (Arc::new(crate::consult::read_only_toolbox(&base)), None)
    } else {
        // Clone the Arc (cheap) rather than move, so `base` stays owned for the
        // layer-2 consult gate below (the gate keys on the merged `base`).
        (Arc::clone(&base), None)
    };

    // Layer 2 (plan §2.3): in consult mode, wrap the configured HotL gate in
    // `ConsultGate` keyed on the FULL base toolbox's read-only set — a
    // hallucinated write-tool name is denied at the gate even though the
    // subset toolbox already hides it.
    // #286: thread the HMAC audit sink + session id through so every consult
    // denial lands in the audit chain as `consult.denied` (best-effort —
    // a missing sink or failed append never weakens the denial).
    let agent_defaults = if mode == TurnMode::Consult {
        crate::consult::consult_agent_config(
            &state.agent_defaults,
            // Key the consult gate on the MERGED base (coding ⑤ + active MCP
            // tools), not the boot `state.toolbox` — otherwise a read-only
            // coding/MCP tool is visible (layer-1) but fail-closed-denied at
            // layer-2 because the gate's allowlist lacks it. Write tools stay
            // denied either way (they're not in the read-only set).
            &base,
            state.hotl_audit.clone(),
            &input.session_id,
        )
    } else {
        state.agent_defaults.clone()
    };

    // 2c''. Attached persona (DEC-036 three-tier model — see `glossary.rs`):
    //       identity (owner, outermost) → team glossary (shared vocabulary) →
    //       persona `system_prompt` (the session's ROLE, innermost of the
    //       three so it sits closest to the task). When the session has a
    //       persona attached and its prompt is non-blank (and the persona is
    //       not archived), inject it as a System message. Inserted at 0 here,
    //       BEFORE the glossary/identity inserts below, so each later
    //       insert-at-0 pushes it down one and the final System order is
    //       [identity, glossary, persona, loop_note?, ...history]. Applies to
    //       Execute, Consult AND loop turns — a persona is the session's role
    //       regardless of mode. Best-effort: a lookup failure is logged and
    //       the turn proceeds without the persona (context enrichment must
    //       never block chat). Like identity/glossary, never persisted into
    //       the session history.
    if let Some(personas) = &state.personas {
        match personas.get_session_persona(&input.session_id).await {
            Ok(Some(persona)) if !persona.archived => {
                for prompt in xiaoguai_personas::build_system_messages(&persona) {
                    messages.insert(0, LlmMessage::system(prompt));
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "persona: session-persona lookup failed (skipping)");
            }
        }
    }

    // 2c'. Team glossary (T7.1): when the session has a team attached and
    //      that team carries a glossary, inject it as a System message
    //      AFTER the identity message (inserted at 0 below, so identity
    //      stays the outermost frame) and BEFORE the history:
    //      [identity, glossary, persona?, loop_note?, ...history]. Applies to
    //      Execute AND Consult turns (read-only context either way) and to
    //      loop ticks — a loop tick belongs to a session like any other turn.
    //      Best-effort: a repo failure is logged and the turn proceeds
    //      without the glossary (context enrichment must not block chat).
    //      Like identity, never persisted into the session history.
    if let Some(teams) = &state.teams {
        match teams.get_session_team(&input.session_id).await {
            Ok(Some(team)) => {
                if let Some(text) = crate::glossary::glossary_system_text(&team) {
                    messages.insert(0, LlmMessage::system(text));
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "glossary: session-team lookup failed (skipping)");
            }
        }
    }

    // 2c. Identity memory (DEC-036, P1): prepend the owner's persistent `USER.md`
    //     profile as a leading System message so every session knows who it is
    //     working for. Loaded per-request (picks up edits without a restart);
    //     absent/blank file → no-op. Not persisted into the session history.
    //     The finalize task persists `outcome.new_messages` (anchored on the
    //     inbound prompt), so no prefix/skip arithmetic is needed here.
    if let Some(identity) = crate::identity::load_identity() {
        messages.insert(0, LlmMessage::system(identity));
    }

    // 3. Build the runtime context.
    //    v0.12.0: every call site builds via RuntimeContext.
    //    L3: thread session + owner attribution so the router records
    //    session-scoped token_usage (also fixes ordinary chat attribution,
    //    not just loop ticks).
    let actor = session.user_id.to_string();
    let model = input.model_override.unwrap_or(session.model);
    let ctx = RuntimeContext::new(state.backend.clone(), toolbox, agent_defaults)
        .with_model(model.clone())
        .with_attribution(Some(input.session_id.clone()), Some(actor.clone()));

    // 4. HOTL budget check — gated on the "llm_call" scope.
    //    Fail-closed: if the enforcer returns Deny, abort before spawning the
    //    agent loop. Escalate is logged and the call proceeds (async review).
    //    When `hotl_enforcer` is None (dev / tests without budget), skip.
    //    An early return here drops `guard`, releasing both the turn lock
    //    and the cancel entry (the pre-extraction code leaked the token).
    if let Some(enforcer) = &state.hotl_enforcer {
        match enforcer.check("llm_call", 1.0).await {
            Ok(crate::hotl::enforcer::HotlVerdict::Allow) => {}
            Ok(crate::hotl::enforcer::HotlVerdict::Escalate(reason)) => {
                tracing::warn!(%reason, "HOTL escalation triggered");
            }
            Ok(crate::hotl::enforcer::HotlVerdict::Deny(reason)) => {
                tracing::warn!(%reason, "HOTL denied LLM call");
                return Err(TurnError::HotlDenied(reason));
            }
            Err(e) => {
                // Enforcer itself errored — fail-closed.
                tracing::error!(?e, "HOTL enforcer error — denying LLM call (fail-closed)");
                return Err(TurnError::HotlUnavailable);
            }
        }
    }

    // 5. Launch the loop via the runtime. `events` closes naturally when
    //    the loop terminates; `join` resolves with the enriched outcome.
    let (join, events) = run_streamed(&ctx, messages, guard.token());

    // 6. Spawn the finalisation task — it runs concurrently with the event
    //    stream and persists anything the loop produced once the join
    //    handle resolves. It owns the turn guard: the lock releases only
    //    after the output is persisted (or the run errored/panicked).
    let (completion_tx, completion) = tokio::sync::oneshot::channel();
    spawn_finalize_task(FinalizeCtx {
        state: state.clone(),
        session_id,
        actor,
        model,
        join,
        guard,
        loop_id: input.loop_id,
        mode,
        completion: completion_tx,
    });

    Ok(TurnHandle {
        events,
        completion,
        loop_intent,
    })
}

/// System note prepended to every loop tick so the agent knows it is one
/// tick of a recurring loop and that the `loop_done` / `loop_pause` tools
/// exist (LLD-LOOP-001 §3 "End condition").
const LOOP_TICK_SYSTEM_NOTE: &str =
    "You are running as one tick of a recurring loop set up by the \
operator. Re-evaluate the task below against the latest state. When the loop's goal has been \
achieved, call the `loop_done` tool with a short reason and write a final summary — no further \
ticks will run. If you are blocked and cannot make progress (e.g. waiting on a human), call \
`loop_pause` with a reason instead. Otherwise, do the work for this tick and stop; the loop will \
run again later.";

/// Inputs to the detached finalisation task. Bundled into one struct so the
/// spawn site stays readable as the audit/identity wiring grows.
struct FinalizeCtx {
    state: AppState,
    session_id: SessionId,
    /// Audit actor — the session owner (`session.user_id`).
    actor: String,
    /// Resolved model for this turn (request override or session default).
    model: String,
    join: tokio::task::JoinHandle<Result<RuntimeOutcome, RuntimeError>>,
    /// Per-session turn lock + cancel entry; released when this task ends.
    guard: TurnGuard,
    /// Loop attribution for the `agent.run` audit entry (`None` = operator).
    loop_id: Option<uuid::Uuid>,
    /// Effective turn mode, stamped as `"mode"` into the `agent.run` audit
    /// details so consult turns are distinguishable in the chain (plan §2.4).
    mode: TurnMode,
    /// Resolves the caller's [`TurnHandle::completion`]. Best-effort: a
    /// dropped receiver (the SSE route drops it) is fine.
    completion: tokio::sync::oneshot::Sender<TurnCompletion>,
}

fn spawn_finalize_task(ctx: FinalizeCtx) {
    let FinalizeCtx {
        state,
        session_id,
        actor,
        model,
        join,
        guard,
        loop_id,
        mode,
        completion,
    } = ctx;
    tokio::spawn(async move {
        let session_id_str = guard.session_id().to_string();
        // Audit-completeness: the SSE chat path runs the agent via
        // `run_streamed` + this detached finaliser, so it never goes through
        // the runtime's audit sink. Emit one HMAC-chained `agent.run` entry
        // per run — completed, errored, or panicked — (same route-level
        // pattern as `hotl.decision`). Best-effort: an audit failure here
        // must NOT affect the already-finished run. Details are content-free
        // by design: counts and enum tags only, never message text or error
        // strings (provider errors can embed response fragments).
        let result = match join.await {
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
                    with_turn_mode(
                        with_loop_attribution(
                            agent_run_details(&model, &outcome, persist_failed),
                            loop_id,
                        ),
                        mode,
                    ),
                )
                .await;
                tracing::info!(
                    stop_reason = ?outcome.stop_reason,
                    iterations = outcome.iterations,
                    "agent run finished"
                );
                let _: StopReason = outcome.stop_reason;
                TurnCompletion::Completed
            }
            Ok(Err(err)) => {
                tracing::error!(?err, "agent run errored");
                append_agent_run_audit(
                    &state,
                    &actor,
                    &session_id_str,
                    with_turn_mode(
                        with_loop_attribution(
                            serde_json::json!({ "model": model, "outcome": "error" }),
                            loop_id,
                        ),
                        mode,
                    ),
                )
                .await;
                TurnCompletion::Errored
            }
            Err(err) => {
                tracing::error!(?err, "agent task panicked");
                append_agent_run_audit(
                    &state,
                    &actor,
                    &session_id_str,
                    with_turn_mode(
                        with_loop_attribution(
                            serde_json::json!({ "model": model, "outcome": "panic" }),
                            loop_id,
                        ),
                        mode,
                    ),
                )
                .await;
                TurnCompletion::Panicked
            }
        };
        if let Err(err) = state.sessions.touch(&session_id_str).await {
            tracing::warn!(?err, "touch session failed");
        }
        // Turn complete — release the per-session lock + cancel entry,
        // then tell the caller how it ended (best-effort).
        drop(guard);
        let _ = completion.send(result);
    });
}

/// Stamp loop attribution into an `agent.run` details payload (LLD-LOOP-001
/// §7, review M3). Operator turns (`loop_id: None`) are unchanged.
fn with_loop_attribution(
    mut details: serde_json::Value,
    loop_id: Option<uuid::Uuid>,
) -> serde_json::Value {
    if let (Some(id), Some(obj)) = (loop_id, details.as_object_mut()) {
        obj.insert("initiator".into(), serde_json::json!("loop"));
        obj.insert("loop_id".into(), serde_json::json!(id.to_string()));
    }
    details
}

/// Stamp the turn's consult/execute mode into the `agent.run` details
/// (T5 plan §2.4). Always present — `"execute"` for ordinary turns keeps the
/// audit chain self-describing.
fn with_turn_mode(mut details: serde_json::Value, mode: TurnMode) -> serde_json::Value {
    if let Some(obj) = details.as_object_mut() {
        obj.insert("mode".into(), serde_json::json!(mode.as_str()));
    }
    details
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

// -- helpers ------------------------------------------------------------

/// Feature ⑤ — resolve the base toolbox for one turn, honouring the session's
/// per-session coding workspace root.
///
/// Returns `state.toolbox` unchanged (the common path) unless ALL of:
///   * the session pins a non-blank `working_dir`,
///   * a [`crate::coding_toolbox::CodingToolboxFactory`] is wired (coding is
///     enabled at boot), AND
///   * that dir differs from the factory's global root.
///
/// Only then is the toolbox rebuilt with the coding tools rooted at the
/// session dir. A rebuild error is logged and falls back to `state.toolbox`:
/// a bad per-session dir must never break the turn (the operator still sees
/// the audit/gate-governed default tools).
async fn resolve_turn_base_toolbox(
    state: &AppState,
    session_working_dir: Option<&str>,
) -> Arc<xiaoguai_agent::Toolbox> {
    let Some(factory) = state.coding_toolbox_factory.as_ref() else {
        return state.toolbox.clone();
    };
    let Some(dir) = session_working_dir.map(str::trim).filter(|s| !s.is_empty()) else {
        return state.toolbox.clone();
    };
    let session_root = std::path::Path::new(dir);
    // A session that pins the global root needs no rebuild — the boot toolbox
    // is already rooted there.
    if factory.global_root() == Some(session_root) {
        return state.toolbox.clone();
    }
    match factory.rebuild_for(session_root).await {
        Ok(tb) => tb,
        Err(err) => {
            tracing::warn!(
                %err,
                working_dir = %dir,
                "feature ⑤: failed to rebuild coding toolbox for session working_dir; \
                 falling back to the boot toolbox"
            );
            state.toolbox.clone()
        }
    }
}

/// Fold every active MCP server's tools onto `base`, dispatched to the owning
/// client, for one turn. Returns a NEW `Arc<Toolbox>` only when at least one
/// MCP tool is actually merged; otherwise `base` is returned unchanged (the
/// common no-active-server path never allocates a toolbox).
///
/// Best-effort and immutable. `source = None` (no supervisor wired) or an
/// empty active set returns `base` untouched. A name collision with a tool
/// already in `base` (coding/loop/demo or another MCP server) is SKIPPED with
/// a `tracing::warn!` — the existing tool is never overwritten and we never
/// panic. Deterministic: the pre-existing `base` entry always wins.
///
/// The merge happens BEFORE the consult/loop layering in [`run_turn`], so the
/// merged MCP tools' `MutationHint` flows through the consult read-only subset
/// and the `ConsultGate` exactly like coding tools — a write MCP tool is
/// hidden+denied in consult mode, a read MCP tool stays available.
async fn merge_active_mcp_tools(
    base: Arc<xiaoguai_agent::Toolbox>,
    source: Option<&dyn ActiveToolsSource>,
) -> Arc<xiaoguai_agent::Toolbox> {
    let Some(source) = source else {
        return base;
    };
    let active = source.active_tools().await;
    if active.is_empty() {
        // Common path: nothing to merge — return `base` without cloning the
        // underlying map.
        return base;
    }
    // Clone-on-merge: never mutate the shared boot/base toolbox.
    let mut merged = (*base).clone();
    for (client, descriptor) in active {
        let name = descriptor.name.clone();
        if let Err(err) = merged.insert(client, descriptor) {
            tracing::warn!(
                tool = %name,
                %err,
                "mcp tool name collides with an existing toolbox tool; skipping (existing tool kept)",
            );
        }
    }
    Arc::new(merged)
}

/// Persist one inbound user message. `pub(crate)` since T4.2: the
/// orchestrate route stores the goal as the session's user message through
/// the exact same path as an ordinary turn.
pub(crate) async fn persist_user_message(
    state: &AppState,
    session_id: &SessionId,
    text: &str,
) -> Result<xiaoguai_types::Message, RepoError> {
    let llm = LlmMessage::user(text);
    let domain = llm_to_domain(session_id, &llm);
    state.messages.append(&domain).await?;
    Ok(domain)
}

async fn load_llm_history(
    state: &AppState,
    session_id: &str,
) -> Result<Vec<LlmMessage>, RepoError> {
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
) -> Result<usize, RepoError> {
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

    #[test]
    fn loop_attribution_stamps_initiator_and_loop_id() {
        let id = uuid::Uuid::new_v4();
        let details = with_loop_attribution(
            serde_json::json!({ "model": "m", "iterations": 1 }),
            Some(id),
        );
        assert_eq!(details["initiator"], "loop");
        assert_eq!(details["loop_id"], id.to_string());
        // Pre-existing keys survive.
        assert_eq!(details["model"], "m");
    }

    #[test]
    fn operator_turns_carry_no_loop_attribution() {
        let details = with_loop_attribution(serde_json::json!({ "model": "m" }), None);
        assert!(details.get("initiator").is_none());
        assert!(details.get("loop_id").is_none());
    }

    #[test]
    fn turn_mode_is_stamped_into_audit_details() {
        let consult = with_turn_mode(serde_json::json!({ "model": "m" }), TurnMode::Consult);
        assert_eq!(consult["mode"], "consult");
        assert_eq!(consult["model"], "m");

        let execute = with_turn_mode(serde_json::json!({}), TurnMode::Execute);
        assert_eq!(execute["mode"], "execute");
    }

    #[test]
    fn turn_mode_serde_is_lowercase_and_defaults_to_execute() {
        let m: TurnMode = serde_json::from_str("\"consult\"").expect("consult parses");
        assert_eq!(m, TurnMode::Consult);
        let m: TurnMode = serde_json::from_str("\"execute\"").expect("execute parses");
        assert_eq!(m, TurnMode::Execute);
        assert_eq!(TurnMode::default(), TurnMode::Execute);
        assert_eq!(serde_json::json!(TurnMode::Consult), "consult");
    }
}

/// MCP live-tools merge ([`merge_active_mcp_tools`]). A mock
/// [`ActiveToolsSource`] + mock `McpClient` exercise the merge without
/// spawning real MCP child processes.
#[cfg(test)]
mod mcp_merge_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{json, Value as JsonValue};
    use std::collections::HashSet;
    use xiaoguai_agent::Toolbox;
    use xiaoguai_mcp::{
        McpClient, McpResult, MutationHint, ServerInfo, ToolDescriptor, ToolResult,
    };

    /// A mock MCP client tagged with an id so a test can assert dispatch went
    /// to the RIGHT client (the merged tools must be dispatched to the MCP
    /// client, not the base toolbox's client).
    struct MockClient {
        id: &'static str,
    }

    #[async_trait]
    impl McpClient for MockClient {
        async fn initialize(&self) -> McpResult<ServerInfo> {
            Ok(ServerInfo {
                name: self.id.into(),
                version: "0".into(),
            })
        }
        async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
            Ok(vec![])
        }
        async fn call_tool(&self, name: &str, _args: JsonValue) -> McpResult<ToolResult> {
            // Echo the client id so a dispatch assertion can prove which
            // client served the call.
            Ok(ToolResult {
                text: format!("{}:{name}", self.id),
                blocks: vec![],
                is_error: false,
            })
        }
        async fn shutdown(&self) -> McpResult<()> {
            Ok(())
        }
    }

    /// Mock source returning a fixed set. An "errored / unavailable
    /// supervisor" is modeled as the source already having swallowed the
    /// failure and returning an empty set (mirrors `McpSupervisor`, which
    /// swallows `list_tools` failures at start so `active_tools` never errors).
    struct MockSource {
        tools: Vec<(Arc<dyn McpClient>, ToolDescriptor)>,
    }

    #[async_trait]
    impl ActiveToolsSource for MockSource {
        async fn active_tools(&self) -> Vec<(Arc<dyn McpClient>, ToolDescriptor)> {
            self.tools.clone()
        }
    }

    fn td(name: &str, hint: MutationHint) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: Some(format!("tool {name}")),
            input_schema: json!({ "type": "object" }),
            mutation_hint: hint,
        }
    }

    fn names(tb: &Toolbox) -> HashSet<String> {
        tb.to_specs().into_iter().map(|s| s.name).collect()
    }

    #[tokio::test]
    async fn merges_read_and_write_tools_dispatched_to_mcp_client() {
        let base = Arc::new(Toolbox::new());
        let mcp: Arc<dyn McpClient> = Arc::new(MockClient { id: "mcp" });
        let source = MockSource {
            tools: vec![
                (mcp.clone(), td("vc_list_vms", MutationHint::Read)),
                (mcp.clone(), td("vc_power_off", MutationHint::Write)),
            ],
        };

        let merged = merge_active_mcp_tools(base, Some(&source)).await;
        assert_eq!(merged.len(), 2);

        let read = merged.get("vc_list_vms").expect("read tool merged");
        assert_eq!(read.descriptor.mutation_hint, MutationHint::Read);
        let write = merged.get("vc_power_off").expect("write tool merged");
        assert_eq!(write.descriptor.mutation_hint, MutationHint::Write);

        // Both dispatch to the MCP client (echo proves the owning client).
        let r = read
            .client
            .call_tool("vc_list_vms", json!({}))
            .await
            .unwrap();
        assert_eq!(r.text, "mcp:vc_list_vms");
        let w = write
            .client
            .call_tool("vc_power_off", json!({}))
            .await
            .unwrap();
        assert_eq!(w.text, "mcp:vc_power_off");
    }

    #[tokio::test]
    async fn merged_write_tool_is_consult_gated() {
        // The write MCP tool must flow through consult's read-only subset
        // (hidden) the same as a coding write tool.
        let base = Arc::new(Toolbox::new());
        let mcp: Arc<dyn McpClient> = Arc::new(MockClient { id: "mcp" });
        let source = MockSource {
            tools: vec![
                (mcp.clone(), td("vc_list_vms", MutationHint::Read)),
                (mcp.clone(), td("vc_power_off", MutationHint::Write)),
            ],
        };
        let merged = merge_active_mcp_tools(base, Some(&source)).await;

        let subset = crate::consult::read_only_toolbox(&merged);
        assert_eq!(names(&subset), HashSet::from(["vc_list_vms".to_string()]));
    }

    #[tokio::test]
    async fn collision_with_existing_tool_is_skipped_not_overwritten() {
        // `base` already owns a tool named `git_status` (its own client).
        let base_client: Arc<dyn McpClient> = Arc::new(MockClient { id: "coding" });
        let mut base_tb = Toolbox::new();
        base_tb
            .insert(base_client, td("git_status", MutationHint::Read))
            .unwrap();
        let base = Arc::new(base_tb);

        // An MCP server advertises a colliding `git_status` plus a fresh tool.
        let mcp: Arc<dyn McpClient> = Arc::new(MockClient { id: "mcp" });
        let source = MockSource {
            tools: vec![
                (mcp.clone(), td("git_status", MutationHint::Write)),
                (mcp.clone(), td("vc_list_vms", MutationHint::Read)),
            ],
        };

        let merged = merge_active_mcp_tools(base, Some(&source)).await;
        // Both names present; the fresh MCP tool landed.
        assert_eq!(
            names(&merged),
            HashSet::from(["git_status".to_string(), "vc_list_vms".to_string()])
        );
        // The pre-existing `git_status` WINS: still dispatched to the base
        // client and still its original Read hint (NOT the MCP server's Write).
        let kept = merged.get("git_status").expect("kept");
        assert_eq!(kept.descriptor.mutation_hint, MutationHint::Read);
        let echo = kept
            .client
            .call_tool("git_status", json!({}))
            .await
            .unwrap();
        assert_eq!(echo.text, "coding:git_status", "base client kept, not MCP");
    }

    #[tokio::test]
    async fn no_active_servers_returns_base_unchanged() {
        let base_client: Arc<dyn McpClient> = Arc::new(MockClient { id: "coding" });
        let mut base_tb = Toolbox::new();
        base_tb
            .insert(base_client, td("git_status", MutationHint::Read))
            .unwrap();
        let base = Arc::new(base_tb);
        let before = Arc::as_ptr(&base);

        let source = MockSource { tools: vec![] };
        let merged = merge_active_mcp_tools(base, Some(&source)).await;
        // Empty active set ⇒ the SAME Arc is returned (no allocation/rebuild).
        assert_eq!(Arc::as_ptr(&merged), before);
        assert_eq!(merged.len(), 1);
    }

    #[tokio::test]
    async fn no_supervisor_returns_base_unchanged() {
        let base = Arc::new(Toolbox::new());
        let before = Arc::as_ptr(&base);
        let merged = merge_active_mcp_tools(base, None).await;
        assert_eq!(Arc::as_ptr(&merged), before);
    }

    #[tokio::test]
    async fn unavailable_source_is_swallowed_and_base_proceeds() {
        // A supervisor whose servers all failed `list_tools` at start surfaces
        // as an empty active set (the supervisor swallowed the error). The
        // turn must proceed with the un-merged base.
        let base_client: Arc<dyn McpClient> = Arc::new(MockClient { id: "coding" });
        let mut base_tb = Toolbox::new();
        base_tb
            .insert(base_client, td("git_status", MutationHint::Read))
            .unwrap();
        let base = Arc::new(base_tb);

        let source = MockSource { tools: vec![] };
        let merged = merge_active_mcp_tools(base, Some(&source)).await;
        assert_eq!(names(&merged), HashSet::from(["git_status".to_string()]));
    }
}
