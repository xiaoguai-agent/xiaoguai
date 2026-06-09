//! REST handlers for `/v1/teams` and `/v1/sessions/:id/team` (T3.2).
//!
//! All routes return 503 Service Unavailable when `AppState.teams` is `None`,
//! preserving the pattern established by personas/memory/hotl routes. The
//! handlers delegate to `xiaoguai_personas::TeamRepository`; boundary checks
//! that need persona data (members must exist and be active) go through
//! `AppState.personas`.
//!
//! ## Routes (mounted in [`crate::routes::router`])
//!
//! | Method | Path                       | Description                                    |
//! |--------|----------------------------|------------------------------------------------|
//! | GET    | `/v1/teams`                | List active teams                              |
//! | POST   | `/v1/teams`                | Create a team                                  |
//! | GET    | `/v1/teams/:id`            | Fetch a team by UUID                           |
//! | PATCH  | `/v1/teams/:id`            | Partial-update a team                          |
//! | DELETE | `/v1/teams/:id`            | Archive (soft-delete) a team                   |
//! | GET    | `/v1/sessions/:id/team`    | Get active team for a session                  |
//! | PUT    | `/v1/sessions/:id/team`    | Attach team (ALSO attaches its lead persona)   |
//! | DELETE | `/v1/sessions/:id/team`    | Detach team (lead persona stays attached)      |
//!
//! Every mutation appends a best-effort `team.*` audit entry via
//! `AppState.team_audit` — audit failure never blocks the operation.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use xiaoguai_personas::teams::model::{CreateTeamRequest, Team, UpdateTeamRequest};
use xiaoguai_personas::{PersonaError, PersonaRepository};

use crate::state::AppState;

// ─── Shared error helpers ────────────────────────────────────────────────────

fn teams_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "teams repository not configured"})),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

fn err_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

fn map_err(e: PersonaError) -> Response {
    match e {
        PersonaError::NotFound => err_response(StatusCode::NOT_FOUND, "not found"),
        PersonaError::DuplicateName(n) => {
            err_response(StatusCode::CONFLICT, format!("duplicate team name: {n}"))
        }
        PersonaError::Archived => err_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "team is archived and cannot be attached",
        ),
        PersonaError::InvalidArgument(msg) => err_response(StatusCode::BAD_REQUEST, msg),
        other => {
            tracing::error!(error = %other, "teams: repository error");
            err_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── Boundary validation ──────────────────────────────────────────────────────

/// Verify every member persona exists and is active. The structural rules
/// (≥1 member, no dupes, lead ∈ members) live in the repository; this is the
/// half that needs persona data, so it runs here at the boundary.
async fn validate_members_active(
    personas: &Arc<dyn PersonaRepository>,
    member_ids: &[Uuid],
) -> Result<(), Response> {
    for id in member_ids {
        match personas.get(*id).await {
            Ok(p) if p.archived => {
                return Err(err_response(
                    StatusCode::BAD_REQUEST,
                    format!("member persona {id} is archived"),
                ));
            }
            Ok(_) => {}
            Err(PersonaError::NotFound) => {
                return Err(err_response(
                    StatusCode::BAD_REQUEST,
                    format!("member persona {id} does not exist"),
                ));
            }
            Err(e) => return Err(map_err(e)),
        }
    }
    Ok(())
}

// ─── Best-effort audit ────────────────────────────────────────────────────────

/// Append a `team.*` audit entry. Failures are logged and discarded — the
/// operation is already persisted and must not be rolled back over telemetry.
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
            tracing::warn!(error = %e, action, "teams: audit append failed (non-blocking)");
        }
    }
}

// ─── Query / body types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AttachTeamBody {
    pub team_id: Uuid,
}

// ─── Team CRUD handlers ───────────────────────────────────────────────────────

pub async fn list_teams(State(state): State<AppState>) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    match repo.list().await {
        Ok(ts) => (StatusCode::OK, Json(ts)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn create_team(
    State(state): State<AppState>,
    Json(body): Json<CreateTeamRequest>,
) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    // Member existence/activeness needs persona data; teams without a
    // persona repo can't be validated, so treat that as unavailable too.
    let Some(personas) = state.personas.clone() else {
        return teams_unavailable();
    };
    if let Err(resp) = validate_members_active(&personas, &body.member_persona_ids).await {
        return resp;
    }
    match repo.create(&body).await {
        Ok(t) => {
            audit(
                &state,
                "team.create",
                format!("team:{}", t.id),
                serde_json::json!({
                    "name": t.name,
                    "lead_persona_id": t.lead_persona_id,
                    "member_count": t.member_persona_ids.len(),
                }),
            )
            .await;
            (StatusCode::CREATED, Json(t)).into_response()
        }
        Err(e) => map_err(e),
    }
}

pub async fn get_team(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    match repo.get(id).await {
        Ok(t) => (StatusCode::OK, Json(t)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn update_team(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTeamRequest>,
) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    let Some(personas) = state.personas.clone() else {
        return teams_unavailable();
    };
    // Only re-check personas when the member list is actually changing.
    if let Some(members) = &body.member_persona_ids {
        if let Err(resp) = validate_members_active(&personas, members).await {
            return resp;
        }
    }
    match repo.update(id, &body).await {
        Ok(t) => {
            audit(
                &state,
                "team.update",
                format!("team:{}", t.id),
                serde_json::json!({"name": t.name}),
            )
            .await;
            (StatusCode::OK, Json(t)).into_response()
        }
        Err(e) => map_err(e),
    }
}

pub async fn archive_team(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    match repo.archive_team(id).await {
        Ok(()) => {
            audit(
                &state,
                "team.archive",
                format!("team:{id}"),
                serde_json::json!({}),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => map_err(e),
    }
}

// ─── Session-attachment handlers ──────────────────────────────────────────────

pub async fn get_session_team(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    match repo.get_session_team(&session_id).await {
        Ok(Some(t)) => (StatusCode::OK, Json(t)).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

/// Attach a team to a session. Until T4 parallel orchestration lands, a team
/// session runs with its **lead persona**, so this handler attaches the lead
/// via the persona path FIRST (it re-validates the lead is active), then
/// records the team attachment. Order matters: if the persona attach fails,
/// no team row is written.
pub async fn attach_team(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<AttachTeamBody>,
) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    let Some(personas) = state.personas.clone() else {
        return teams_unavailable();
    };

    let team: Team = match repo.get(body.team_id).await {
        Ok(t) => t,
        Err(e) => return map_err(e),
    };
    if team.archived {
        return map_err(PersonaError::Archived);
    }

    if let Err(e) = personas
        .attach_persona_to_session(&session_id, team.lead_persona_id)
        .await
    {
        return map_err(e);
    }
    match repo.attach_team_to_session(&session_id, body.team_id).await {
        Ok(st) => {
            audit(
                &state,
                "team.attach",
                format!("team:{}", body.team_id),
                serde_json::json!({
                    "session_id": session_id,
                    "lead_persona_id": team.lead_persona_id,
                }),
            )
            .await;
            (StatusCode::OK, Json(st)).into_response()
        }
        Err(e) => map_err(e),
    }
}

/// Detach the team from a session. Deliberately leaves the lead persona
/// attached — removing the expert is the explicit
/// `DELETE /v1/sessions/:id/persona`.
pub async fn detach_team(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response {
    let Some(repo) = state.teams.clone() else {
        return teams_unavailable();
    };
    match repo.detach_team_from_session(&session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}
