//! REST handlers for `/v1/personas` and `/v1/sessions/:id/persona`.
//!
//! All routes return 503 Service Unavailable when `AppState.personas` is
//! `None`, preserving the pattern established by memory, hotl, outcomes, and
//! `skill_packs` routes. Production wires a `PgPersonaRepository` from
//! `xiaoguai-personas::pg`.
//!
//! ## Routes (mounted in [`crate::routes::router`])
//!
//! | Method | Path                              | Description                              |
//! |--------|-----------------------------------|------------------------------------------|
//! | GET    | `/v1/personas?tenant_id=<uuid>`   | List active personas for the tenant      |
//! | POST   | `/v1/personas`                    | Create a persona                         |
//! | GET    | `/v1/personas/:id`                | Fetch a persona by UUID                  |
//! | PATCH  | `/v1/personas/:id`                | Partial-update a persona                 |
//! | DELETE | `/v1/personas/:id`                | Archive (soft-delete) a persona          |
//! | GET    | `/v1/sessions/:id/persona`        | Get active persona for a session         |
//! | PUT    | `/v1/sessions/:id/persona`        | Attach / replace persona for a session   |
//! | DELETE | `/v1/sessions/:id/persona`        | Detach persona from session              |
//!
//! The handlers delegate to `xiaoguai_personas::PersonaRepository` —
//! storage details never leak through this layer.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use xiaoguai_personas::{CreatePersonaRequest, PersonaError, UpdatePersonaRequest};

use crate::state::AppState;

// ─── Shared error helpers ────────────────────────────────────────────────────

fn personas_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "personas repository not configured"})),
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
            err_response(StatusCode::CONFLICT, format!("duplicate persona name: {n}"))
        }
        PersonaError::Archived => err_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "persona is archived and cannot be attached",
        ),
        PersonaError::InvalidArgument(msg) => err_response(StatusCode::BAD_REQUEST, msg),
        other => {
            tracing::error!(error = %other, "personas: repository error");
            err_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── Query / body types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TenantQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct AttachBody {
    pub persona_id: Uuid,
}

// ─── Persona CRUD handlers ────────────────────────────────────────────────────

pub async fn list_personas(
    State(state): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.list(q.tenant_id).await {
        Ok(ps) => (StatusCode::OK, Json(ps)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn create_persona(
    State(state): State<AppState>,
    Json(body): Json<CreatePersonaRequest>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.create(&body).await {
        Ok(p) => (StatusCode::CREATED, Json(p)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn get_persona(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.get(id).await {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn update_persona(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePersonaRequest>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.update(id, &body).await {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn archive_persona(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.archive_persona(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

// ─── Session-attachment handlers ──────────────────────────────────────────────

pub async fn get_session_persona(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.get_session_persona(&session_id).await {
        Ok(Some(p)) => (StatusCode::OK, Json(p)).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn attach_persona(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<AttachBody>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo
        .attach_persona_to_session(&session_id, body.persona_id)
        .await
    {
        Ok(sp) => (StatusCode::OK, Json(sp)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn detach_persona(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response {
    let Some(repo) = state.personas.clone() else {
        return personas_unavailable();
    };
    match repo.detach_persona_from_session(&session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}
