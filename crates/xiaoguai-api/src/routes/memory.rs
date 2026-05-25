//! REST handlers for `/v1/memories`.
//!
//! All routes return 503 Service Unavailable when `AppState.memory_store` is
//! `None`, preserving the pattern established by hotl, outcomes, and
//! skill_packs routes.
//!
//! ## Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET    | `/v1/memories` | List memories for the authenticated tenant |
//! | POST   | `/v1/memories` | Create a new memory |
//! | GET    | `/v1/memories/:id` | Fetch one memory |
//! | PUT    | `/v1/memories/:id` | Update a memory |
//! | DELETE | `/v1/memories/:id` | Delete a memory |
//! | POST   | `/v1/memories/recall` | Semantic recall by natural-language query |
//! | GET    | `/v1/memories/similar/:id` | Find memories similar to `id` |

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use xiaoguai_memory::{
    types::{CreateMemoryRequest, RecallRequest, UpdateMemoryRequest},
    MemoryKind,
};

use crate::state::AppState;

// ─── Shared error helper ─────────────────────────────────────────────────────

fn memory_unavailable() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "memory_store not configured"})),
    )
}

fn not_found(id: Uuid) -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": format!("memory not found: {id}")})),
    )
}

fn internal(msg: impl std::fmt::Display) -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": msg.to_string()})),
    )
}

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListMemoriesQuery {
    pub tenant_id: Uuid,
    pub kind: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub struct SimilarQuery {
    pub tenant_id: Uuid,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    5
}

// ─── Request bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateMemoryBody {
    pub tenant_id: Uuid,
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub ttl_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryBody {
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    /// `null` removes the TTL; omitting the field leaves it unchanged.
    pub ttl_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
}

#[derive(Debug, Deserialize)]
pub struct RecallBody {
    pub tenant_id: Uuid,
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    pub kind_filter: Option<String>,
    #[serde(default)]
    pub tag_filter: Vec<String>,
    pub session_id: Option<Uuid>,
}

// ─── Response wrapper ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct MemoryResponse<T: Serialize> {
    pub data: T,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/memories?tenant_id=&kind=&tags=&limit=&offset=`
pub async fn list_memories(
    State(state): State<AppState>,
    Query(q): Query<ListMemoriesQuery>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let kind_filter = if let Some(k) = q.kind {
        match k.parse::<MemoryKind>() {
            Ok(kind) => Some(kind),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("unknown kind: {k}")})),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    match store
        .list_memories(q.tenant_id, kind_filter, &q.tags, q.limit, q.offset)
        .await
    {
        Ok(memories) => Json(MemoryResponse { data: memories }).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `GET /v1/memories/:id?tenant_id=`
pub async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let tenant_id = match params.get("tenant_id").and_then(|s| s.parse::<Uuid>().ok()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "tenant_id required"})),
            )
                .into_response();
        }
    };

    match store.get_memory(id, tenant_id).await {
        Ok(m) => Json(MemoryResponse { data: m }).into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `POST /v1/memories`
pub async fn create_memory(
    State(state): State<AppState>,
    Json(body): Json<CreateMemoryBody>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let kind = match body.kind.parse::<MemoryKind>() {
        Ok(k) => k,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("unknown kind: {}", body.kind)})),
            )
                .into_response();
        }
    };

    let req = CreateMemoryRequest {
        tenant_id: body.tenant_id,
        kind,
        content: body.content,
        tags: body.tags,
        ttl_at: body.ttl_at,
    };

    match store.create_memory(req).await {
        Ok(m) => (StatusCode::CREATED, Json(MemoryResponse { data: m })).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `PUT /v1/memories/:id?tenant_id=`
pub async fn update_memory(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<UpdateMemoryBody>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let tenant_id = match params.get("tenant_id").and_then(|s| s.parse::<Uuid>().ok()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "tenant_id required"})),
            )
                .into_response();
        }
    };

    let req = UpdateMemoryRequest {
        content: body.content,
        tags: body.tags,
        ttl_at: body.ttl_at,
    };

    match store.update_memory(id, tenant_id, req).await {
        Ok(m) => Json(MemoryResponse { data: m }).into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `DELETE /v1/memories/:id?tenant_id=`
pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let tenant_id = match params.get("tenant_id").and_then(|s| s.parse::<Uuid>().ok()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "tenant_id required"})),
            )
                .into_response();
        }
    };

    match store.delete_memory(id, tenant_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `POST /v1/memories/recall`
pub async fn recall_memories(
    State(state): State<AppState>,
    Json(body): Json<RecallBody>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let kind_filter = if let Some(k) = body.kind_filter {
        match k.parse::<MemoryKind>() {
            Ok(kind) => Some(kind),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("unknown kind filter: {k}")})),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    let req = RecallRequest {
        tenant_id: body.tenant_id,
        query: body.query,
        top_k: body.top_k,
        kind_filter,
        tag_filter: body.tag_filter,
        session_id: body.session_id,
    };

    match store.recall_memories(req).await {
        Ok(recalled) => Json(MemoryResponse { data: recalled }).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `GET /v1/memories/similar/:id?tenant_id=&top_k=`
pub async fn find_similar(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<SimilarQuery>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    match store.find_similar(id, q.tenant_id, q.top_k).await {
        Ok(similar) => Json(MemoryResponse { data: similar }).into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        Err(e) => internal(e).into_response(),
    }
}
