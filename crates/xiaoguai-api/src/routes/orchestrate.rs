//! `POST /v1/sessions/:id/orchestrate` — team execution as one session
//! turn (T4.2 of `docs/plans/2026-06-10-executive-orchestration.md` §2.2).
//!
//! Goal in → members run in parallel → lead synthesizes one answer out,
//! streamed as SSE [`ExecEvent`] frames. The orchestrated run IS the
//! session's turn:
//!
//! - holds the per-session turn lock for the whole run (409
//!   `turn_in_flight` on collision — same wire shape as `send_message`);
//! - persists the goal as the user message and the synthesized text as
//!   the assistant reply (only the synthesis is persisted; member
//!   transcripts surface via SSE + attribution + audit, plan §0);
//! - `HotL` turn gate up front: `enforcer.check("llm_call", members + 1)`,
//!   fail-closed like `run_turn`; per-tool gates apply inside each member
//!   run automatically (the gate rides in on `agent_defaults`);
//! - audits `orchestration.start` / `orchestration.complete` through the
//!   `team_audit` sink (best-effort, never blocks the run);
//! - completes + persists even if the SSE client disconnects: the run is
//!   driven by a detached task that owns the turn guard, mirroring
//!   `run_turn`'s detached finalize.
//!
//! Routing: an explicit `team_id` wins; otherwise the goal is auto-routed
//! to the top team from the T3 suggest scorer (`routes::experts`), 422
//! when nothing matches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use futures::StreamExt;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use xiaoguai_orchestrator::patterns::executive::{
    ExecEvent, ExecutiveRunner, MemberSpec, DEFAULT_MAX_MEMBERS,
};
use xiaoguai_personas::teams::model::Team;
use xiaoguai_personas::Persona;
use xiaoguai_types::{SessionId, SessionStatus};

use crate::error::ApiError;
use crate::orchestrate::OrchestrateMemberRunner;
use crate::routes::experts::{score_team, tokenize};
use crate::state::{AppState, TurnGuard};

/// Matches the executive runner's worst case (`2 × members + 3` events)
/// with room to spare; a slow SSE consumer backpressures the forwarder,
/// a dropped one is ignored (best-effort send).
const SSE_CHANNEL_CAPACITY: usize = 64;

/// #285: effective per-request member cap. [`DEFAULT_MAX_MEMBERS`] is the
/// engine's hard resource ceiling — a request may LOWER the cap but never
/// raise it (parallel fan-out stays bounded); the floor is lifted to 1 so
/// the constructor's cap check stays meaningful. Pure — unit-tested below.
fn effective_max_members(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_MAX_MEMBERS)
        .clamp(1, DEFAULT_MAX_MEMBERS)
}

#[derive(Debug, Deserialize)]
pub struct OrchestrateRequest {
    pub goal: String,
    /// Explicit team. `None` = auto-route the goal via the suggest scorer.
    pub team_id: Option<Uuid>,
    /// Per-request member cap; defaults to the engine cap (8). #285: the
    /// value is clamped to `1..=DEFAULT_MAX_MEMBERS` — a request can lower
    /// the cap but never raise it above the engine ceiling.
    pub max_members: Option<usize>,
}

// ─── Error helpers (teams.rs conventions; 409/503 reuse ApiError so the
//     wire shape matches send_message exactly) ───────────────────────────────

// DEC-041: helpers map onto the canonical crate::error::ApiError (same
// {code,message} wire shape as send_message), matching the rest of this file.
fn unavailable(what: &str) -> Response {
    ApiError::ServiceUnavailable(format!("{what} repository not configured")).into_response()
}

fn unprocessable(msg: impl Into<String>) -> Response {
    ApiError::Unprocessable(msg.into()).into_response()
}

// ─── Best-effort audit (team_audit sink pattern, teams.rs) ───────────────────

async fn audit(state: &AppState, action: &str, resource: String, details: serde_json::Value) {
    if let Some(sink) = &state.team_audit {
        let entry = xiaoguai_audit::AuditEntry {
            ts: Utc::now(),
            tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
            actor: "owner".to_string(),
            action: action.to_string(),
            resource: Some(resource),
            details,
        };
        if let Err(e) = sink.append(entry).await {
            tracing::warn!(error = %e, action, "orchestrate: audit append failed (non-blocking)");
        }
    }
}

// ─── Auto-routing (pure; reuses the T3 suggest scorer verbatim) ──────────────

/// Rank `teams` against the goal with [`score_team`] and return the top
/// match. Zero-score teams are dropped; ties break by name (same ordering
/// contract as `/v1/experts/suggest`). `None` when nothing matches.
fn pick_team_for_goal<'a>(goal: &str, teams: &'a [Team], personas: &[Persona]) -> Option<&'a Team> {
    let goal_tokens = tokenize(goal);
    if goal_tokens.is_empty() {
        return None;
    }
    teams
        .iter()
        .filter_map(|t| {
            let members: Vec<&Persona> = personas
                .iter()
                .filter(|p| t.member_persona_ids.contains(&p.id))
                .collect();
            let score = score_team(&goal_tokens, t, &members);
            (score > 0).then_some((score, t))
        })
        // max_by prefers later elements on ties, so order by
        // (score, Reverse(name)) to keep "ties break by name ascending".
        .max_by(|(sa, ta), (sb, tb)| sa.cmp(sb).then_with(|| tb.name.cmp(&ta.name)))
        .map(|(_, t)| t)
}

// ─── SSE encoding ─────────────────────────────────────────────────────────────

/// Encode one [`ExecEvent`] as an SSE frame, mirroring `event_to_sse_seq`
/// (`crate::sse`): `event:` carries the variant tag, `data:` the serde-JSON
/// body, `id:` the per-stream monotonic sequence number. The variant tag is
/// read back out of the serialized body (`type` field) so the SSE name can
/// never drift from the serde contract.
fn exec_event_to_sse(ev: &ExecEvent, seq: u64) -> Event {
    let json = serde_json::to_value(ev).unwrap_or_else(
        |e| serde_json::json!({"type": "error", "message": format!("encode: {e}")}),
    );
    let name = json
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error")
        .to_string();
    Event::default()
        .event(name)
        .data(json.to_string())
        .id(seq.to_string())
}

// ─── Handler ──────────────────────────────────────────────────────────────────

/// `POST /v1/sessions/:id/orchestrate` — run the goal through a team and
/// stream [`ExecEvent`]s as SSE. See the module docs for the full contract.
#[allow(
    clippy::too_many_lines,
    reason = "linear turn pipeline, mirrors run_turn"
)]
pub async fn orchestrate_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<OrchestrateRequest>,
) -> Response {
    // 503-when-absent: orchestration needs both personas and teams.
    let Some(personas_repo) = state.personas.clone() else {
        return unavailable("personas");
    };
    let Some(teams_repo) = state.teams.clone() else {
        return unavailable("teams");
    };

    if req.goal.trim().is_empty() {
        return unprocessable("goal must not be blank");
    }

    // Session must exist and be active (same contract as send_message).
    let session = match state.sessions.find_by_id(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return ApiError::NotFound.into_response(),
        Err(e) => return ApiError::Storage(e).into_response(),
    };
    if !matches!(session.status, SessionStatus::Active) {
        return ApiError::Conflict("session is not active".into()).into_response();
    }

    // Active personas — needed for both routing and member resolution.
    let personas = match personas_repo.list().await {
        Ok(ps) => ps,
        Err(e) => {
            tracing::error!(error = %e, "orchestrate: persona list failed");
            return ApiError::Internal(anyhow::anyhow!("persona list: {e}")).into_response();
        }
    };

    // Routing: explicit team_id wins; otherwise auto-route via the T3
    // suggest scorer (422 when nothing matches, owner decision §5.1).
    let team: Team = if let Some(id) = req.team_id {
        match teams_repo.get(id).await {
            Ok(t) if t.archived => return unprocessable("team is archived"),
            Ok(t) => t,
            Err(xiaoguai_personas::PersonaError::NotFound) => {
                return unprocessable(format!("unknown team: {id}"));
            }
            Err(e) => {
                tracing::error!(error = %e, "orchestrate: team get failed");
                return ApiError::Internal(anyhow::anyhow!("team get: {e}")).into_response();
            }
        }
    } else {
        let teams = match teams_repo.list().await {
            Ok(ts) => ts,
            Err(e) => {
                tracing::error!(error = %e, "orchestrate: team list failed");
                return ApiError::Internal(anyhow::anyhow!("team list: {e}")).into_response();
            }
        };
        match pick_team_for_goal(&req.goal, &teams, &personas) {
            Some(t) => t.clone(),
            None => return unprocessable("no team matches the goal"),
        }
    };

    // Resolve active member personas in team order; archived/missing
    // members are dropped (continue-with-survivors starts at resolution).
    let by_id: HashMap<Uuid, Persona> = personas.into_iter().map(|p| (p.id, p)).collect();
    let member_personas: Vec<Persona> = team
        .member_persona_ids
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect();
    if member_personas.is_empty() {
        return unprocessable("team has no active member personas");
    }
    let Some(lead_persona) = by_id.get(&team.lead_persona_id).cloned() else {
        return unprocessable("team lead persona is archived or missing");
    };

    // HotL turn gate — one llm_call per member plus the synthesis turn.
    // Fail-closed, same as run_turn (Deny and enforcer errors both refuse).
    if let Some(enforcer) = &state.hotl_enforcer {
        let amount = f64::from(u32::try_from(member_personas.len() + 1).unwrap_or(u32::MAX));
        match enforcer.check("llm_call", amount).await {
            Ok(crate::hotl::enforcer::HotlVerdict::Allow) => {}
            Ok(crate::hotl::enforcer::HotlVerdict::Escalate(reason)) => {
                tracing::warn!(%reason, "HOTL escalation triggered (orchestrate)");
            }
            Ok(crate::hotl::enforcer::HotlVerdict::Deny(reason)) => {
                tracing::warn!(%reason, "HOTL denied orchestrated run");
                return ApiError::ServiceUnavailable(format!(
                    "LLM call denied by HOTL policy: {reason}"
                ))
                .into_response();
            }
            Err(e) => {
                tracing::error!(
                    ?e,
                    "HOTL enforcer error — denying orchestrated run (fail-closed)"
                );
                return ApiError::ServiceUnavailable(
                    "LLM call denied: HOTL enforcer unavailable".into(),
                )
                .into_response();
            }
        }
    }

    // Per-session turn lock — an orchestrated run IS the session's turn.
    // Same 409 wire shape as send_message (`turn_error_to_api`).
    let Some(guard) = state.cancels.try_begin_turn(&session_id) else {
        return ApiError::Conflict("a turn is already in flight for this session".into())
            .into_response();
    };

    // Build the agent-backed runner + the executive engine. #285: this is
    // pure construction-time validation (member cap, lead membership) and
    // runs BEFORE the goal is persisted, so a 422 here leaves no orphan
    // user message behind.
    let run_id = Uuid::new_v4();
    let actor = session.user_id.to_string();
    let mut persona_map: HashMap<Uuid, Persona> =
        member_personas.iter().map(|p| (p.id, p.clone())).collect();
    persona_map.insert(lead_persona.id, lead_persona.clone());
    let runner = Arc::new(OrchestrateMemberRunner::new(
        state.backend.clone(),
        state.toolbox.clone(),
        state.agent_defaults.clone(),
        persona_map,
        session.model.clone(),
        actor,
        run_id,
        guard.token(),
        // T7.1: team glossary rides into every member + synthesis run.
        crate::glossary::glossary_system_text(&team),
    ));
    let members: Vec<MemberSpec> = member_personas
        .iter()
        .map(|p| MemberSpec {
            id: p.id,
            name: p.name.clone(),
        })
        .collect();
    let lead = MemberSpec {
        id: lead_persona.id,
        name: lead_persona.name.clone(),
    };
    // #285: clamp — a request may lower the cap but never raise it above
    // the engine ceiling (parallel fan-out stays bounded).
    let max_members = effective_max_members(req.max_members);
    let executive = match ExecutiveRunner::with_max_members(runner, lead, members, max_members) {
        Ok(e) => e,
        Err(e) => return unprocessable(e.to_string()),
    };

    // Persist the goal as the user message (shared turn.rs helper) — only
    // after all construction-time validation passed (#285).
    let typed_session_id = SessionId::from(session_id.clone());
    if let Err(e) = crate::turn::persist_user_message(&state, &typed_session_id, &req.goal).await {
        return ApiError::Storage(e).into_response();
    }

    audit(
        &state,
        "orchestration.start",
        format!("session:{session_id}"),
        serde_json::json!({
            "team_id": team.id,
            "member_count": member_personas.len(),
            "run_id": run_id,
        }),
    )
    .await;

    // Detached run: the forwarder owns the turn guard and drives the
    // executive stream to completion regardless of the SSE client. The
    // route only holds the channel's receive side — a dropped client makes
    // the forwards no-ops while the run still persists + audits + releases
    // the lock (mirrors run_turn's detached finalize).
    let (tx, rx) = tokio::sync::mpsc::channel::<ExecEvent>(SSE_CHANNEL_CAPACITY);
    spawn_run_forwarder(RunCtx {
        state: state.clone(),
        session_id,
        typed_session_id,
        run_id,
        guard,
        events: executive.stream(req.goal),
        tx,
    });

    let sse_stream = ReceiverStream::new(rx)
        .enumerate()
        .map(|(i, ev)| Ok::<_, axum::Error>(exec_event_to_sse(&ev, i as u64 + 1)));
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

// ─── Detached run forwarder + finalize ────────────────────────────────────────

/// Inputs to the detached forwarder task (bundled like `turn::FinalizeCtx`).
struct RunCtx<S> {
    state: AppState,
    session_id: String,
    typed_session_id: SessionId,
    run_id: Uuid,
    /// Per-session turn lock + cancel entry; released when the task ends.
    guard: TurnGuard,
    events: S,
    tx: tokio::sync::mpsc::Sender<ExecEvent>,
}

fn spawn_run_forwarder<S>(ctx: RunCtx<S>)
where
    S: futures::Stream<Item = ExecEvent> + Send + 'static,
{
    let RunCtx {
        state,
        session_id,
        typed_session_id,
        run_id,
        guard,
        events,
        tx,
    } = ctx;
    tokio::spawn(async move {
        let mut events = Box::pin(events);
        while let Some(ev) = events.next().await {
            // Final carries the run's terminal state: persist the
            // synthesized text (success only — a failure text is an error
            // message, not an answer) and append the completion audit
            // BEFORE forwarding, so a client that saw the stream end can
            // rely on the message being durable.
            if let ExecEvent::Final {
                ok,
                text,
                failed_members,
            } = &ev
            {
                if *ok {
                    let assistant = xiaoguai_llm::Message::assistant(text);
                    let domain = crate::convert::llm_to_domain(&typed_session_id, &assistant);
                    if let Err(err) = state.messages.append(&domain).await {
                        tracing::error!(?err, "orchestrate: failed to persist synthesized reply");
                    }
                }
                audit(
                    &state,
                    "orchestration.complete",
                    format!("session:{session_id}"),
                    serde_json::json!({
                        "ok": ok,
                        "failed_members": failed_members,
                        "run_id": run_id,
                    }),
                )
                .await;
            }
            // Best-effort forward — a disconnected SSE client never stops
            // the run (same convention as the executive runner's `emit`).
            let _ = tx.send(ev).await;
        }
        if let Err(err) = state.sessions.touch(&session_id).await {
            tracing::warn!(?err, "orchestrate: touch session failed");
        }
        // Run complete — release the per-session lock + cancel entry.
        drop(guard);
    });
}

// ─── Unit tests (pure functions) ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn persona(name: &str) -> Persona {
        Persona {
            id: Uuid::new_v4(),
            name: name.to_string(),
            system_prompt: format!("You are {name}."),
            default_model: None,
            tool_allowlist: None,
            escalation_tier: None,
            created_at: Utc::now(),
            archived: false,
        }
    }

    fn team(name: &str, description: &str, members: &[&Persona]) -> Team {
        Team {
            id: Uuid::new_v4(),
            name: name.to_string(),
            description: description.to_string(),
            lead_persona_id: members[0].id,
            member_persona_ids: members.iter().map(|p| p.id).collect(),
            recommended_pack_slugs: vec![],
            glossary_md: None,
            created_at: Utc::now(),
            archived: false,
        }
    }

    #[test]
    fn pick_team_prefers_higher_score() {
        let analyst = persona("Finance Analyst");
        let writer = persona("Copy Writer");
        let finance = team("Finance Squad", "quarterly finance reports", &[&analyst]);
        let copy = team("Copy Desk", "marketing copy", &[&writer]);
        let teams = vec![copy, finance.clone()];
        let personas = vec![analyst, writer];

        let picked = pick_team_for_goal("analyse the finance report", &teams, &personas)
            .expect("a team matches");
        assert_eq!(picked.id, finance.id);
    }

    #[test]
    fn pick_team_returns_none_when_nothing_matches() {
        let p = persona("Gardener");
        let t = team("Garden Crew", "pruning roses", &[&p]);
        assert!(pick_team_for_goal("quantum chromodynamics", &[t], &[p]).is_none());
    }

    #[test]
    fn pick_team_returns_none_for_blank_goal() {
        let p = persona("Anyone");
        let t = team("Any Team", "anything", &[&p]);
        assert!(pick_team_for_goal("   ", &[t], &[p]).is_none());
    }

    #[test]
    fn pick_team_ties_break_by_name_ascending() {
        // Two teams with identical token overlap — the suggest scorer's
        // ordering contract (name ascending) must hold here too.
        let a = persona("A");
        let b = persona("B");
        let t_beta = team("beta finance", "", &[&b]);
        let t_alpha = team("alpha finance", "", &[&a]);
        let teams = vec![t_beta, t_alpha.clone()];
        let picked = pick_team_for_goal("finance", &teams, &[a, b]).expect("both match");
        assert_eq!(picked.id, t_alpha.id, "tie must break by name ascending");
    }

    // #285: the request cap may lower but never raise the engine ceiling.
    #[test]
    fn effective_max_members_defaults_to_engine_cap() {
        assert_eq!(effective_max_members(None), DEFAULT_MAX_MEMBERS);
    }

    #[test]
    fn effective_max_members_clamps_oversized_request_to_engine_cap() {
        assert_eq!(effective_max_members(Some(1000)), DEFAULT_MAX_MEMBERS);
        assert_eq!(
            effective_max_members(Some(DEFAULT_MAX_MEMBERS + 1)),
            DEFAULT_MAX_MEMBERS
        );
    }

    #[test]
    fn effective_max_members_lifts_zero_to_one() {
        assert_eq!(effective_max_members(Some(0)), 1);
    }

    #[test]
    fn effective_max_members_keeps_in_range_request() {
        assert_eq!(effective_max_members(Some(3)), 3);
        assert_eq!(
            effective_max_members(Some(DEFAULT_MAX_MEMBERS)),
            DEFAULT_MAX_MEMBERS
        );
    }

    #[test]
    fn exec_event_sse_name_matches_serde_tag() {
        let ev = ExecEvent::RunStarted { members: 3 };
        let sse = exec_event_to_sse(&ev, 7);
        let rendered = format!("{sse:?}");
        assert!(rendered.contains("run_started"), "got {rendered}");
        assert!(rendered.contains('7'), "seq id stamped: {rendered}");
    }

    #[test]
    fn exec_event_sse_final_carries_payload() {
        let id = Uuid::new_v4();
        let ev = ExecEvent::Final {
            ok: true,
            text: "synth".to_string(),
            failed_members: vec![id],
        };
        let rendered = format!("{:?}", exec_event_to_sse(&ev, 1));
        assert!(rendered.contains("final"), "got {rendered}");
        assert!(rendered.contains("synth"), "got {rendered}");
        assert!(rendered.contains(&id.to_string()), "got {rendered}");
    }
}
