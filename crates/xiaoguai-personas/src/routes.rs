//! REST route definitions for the personas API.
//!
//! These handlers are designed to be mounted by `xiaoguai-core` or
//! `xiaoguai-api` — they depend only on the `PersonaRepository` trait so the
//! actual storage backend is injected by the wiring layer.
//!
//! ## Endpoints
//!
//! | Method | Path                                   | Description                              |
//! |--------|----------------------------------------|------------------------------------------|
//! | GET    | `/v1/personas`                         | List active personas for the tenant      |
//! | POST   | `/v1/personas`                         | Create a persona                         |
//! | GET    | `/v1/personas/:id`                     | Fetch a persona by UUID                  |
//! | PATCH  | `/v1/personas/:id`                     | Partial-update a persona                 |
//! | DELETE | `/v1/personas/:id`                     | Archive (soft-delete) a persona          |
//! | PUT    | `/v1/sessions/:id/persona`             | Attach / replace persona for a session   |
//! | DELETE | `/v1/sessions/:id/persona`             | Detach persona from session              |
//! | GET    | `/v1/sessions/:id/persona`             | Get active persona for a session         |

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::PersonaError;
use crate::model::{CreatePersonaRequest, UpdatePersonaRequest};
use crate::traits::PersonaRepository;

/// Shared state injected by the wiring layer. Kept minimal — just the repo.
#[derive(Clone)]
pub struct PersonaApiState {
    pub repo: Arc<dyn PersonaRepository>,
}

/// Build the axum `Router` fragment. Mount at the workspace's `/v1` prefix:
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use xiaoguai_personas::{InMemoryPersonaRepository, routes};
/// let state = routes::PersonaApiState {
///     repo: Arc::new(InMemoryPersonaRepository::new()),
/// };
/// let _router = routes::router(state);
/// ```
pub fn router(state: PersonaApiState) -> Router {
    Router::new()
        .route("/v1/personas", get(list_personas).post(create_persona))
        .route(
            "/v1/personas/:id",
            get(get_persona)
                .patch(update_persona)
                .delete(archive_persona),
        )
        .route(
            "/v1/sessions/:id/persona",
            get(get_session_persona)
                .put(attach_persona)
                .delete(detach_persona),
        )
        .with_state(state)
}

// ── Request / response bodies ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TenantQuery {
    tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct AttachBody {
    persona_id: Uuid,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

/// Build a concrete `Response` so every match arm in `map_err` has the same type.
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

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn list_personas(
    State(s): State<PersonaApiState>,
    axum::extract::Query(q): axum::extract::Query<TenantQuery>,
) -> impl IntoResponse {
    match s.repo.list(q.tenant_id).await {
        Ok(ps) => (StatusCode::OK, Json(ps)).into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn create_persona(
    State(s): State<PersonaApiState>,
    Json(body): Json<CreatePersonaRequest>,
) -> impl IntoResponse {
    match s.repo.create(&body).await {
        Ok(p) => (StatusCode::CREATED, Json(p)).into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn get_persona(State(s): State<PersonaApiState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    match s.repo.get(id).await {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn update_persona(
    State(s): State<PersonaApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePersonaRequest>,
) -> impl IntoResponse {
    match s.repo.update(id, &body).await {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn archive_persona(
    State(s): State<PersonaApiState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match s.repo.archive_persona(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn get_session_persona(
    State(s): State<PersonaApiState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match s.repo.get_session_persona(&session_id).await {
        Ok(Some(p)) => (StatusCode::OK, Json(p)).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn attach_persona(
    State(s): State<PersonaApiState>,
    Path(session_id): Path<String>,
    Json(body): Json<AttachBody>,
) -> impl IntoResponse {
    match s
        .repo
        .attach_persona_to_session(&session_id, body.persona_id)
        .await
    {
        Ok(sp) => (StatusCode::OK, Json(sp)).into_response(),
        Err(e) => map_err(e).into_response(),
    }
}

async fn detach_persona(
    State(s): State<PersonaApiState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match s.repo.detach_persona_from_session(&session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e).into_response(),
    }
}
