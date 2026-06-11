//! REST handlers for `/v1/memories`.
//!
//! All routes return 503 Service Unavailable when `AppState.memory_store` is
//! `None`, preserving the pattern established by hotl, outcomes, and
//! `skill_packs` routes.
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
//! | GET    | `/v1/memories/export?kind=` | Export memories as JSONL (T7.2) |
//! | POST   | `/v1/memories/import` | Import a JSONL body, fail-soft (T7.2) |
//!
//! ## Source-tag convention (T7.2, plan §1.2)
//!
//! Tags prefixed `source:` record where a memory came from — pure
//! convention over the existing `tags` column, no schema: `source:imported`
//! (added automatically by the import path unless the line already carries
//! a `source:` tag), `source:im`, `source:rag`. Recall/list tag filtering
//! works on them like any other tag. Codec + tagging live in
//! [`xiaoguai_memory::jsonl`] so the CLI shares them.
//!
//! Import/export audit via the generic `team_audit` sink (same pattern as
//! incidents): best-effort `memory.export` / `memory.import` entries with
//! counts only — never memory content.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use xiaoguai_memory::{
    jsonl,
    types::{CreateMemoryRequest, RecallRequest, UpdateMemoryRequest},
    MemoryKind,
};

use crate::state::AppState;

/// Explicit body limit for `POST /v1/memories/import` (#288): 8 MiB
/// replaces axum's 2 MiB default — large enough for ~10k JSONL lines
/// (the import loop's own `MAX_IMPORT_LINES` cap) while still bounding a
/// single request. Libraries bigger than this should go through the CLI
/// (`xiaoguai memory import`), which talks to the local store directly.
/// Applied via `DefaultBodyLimit::max` on the route in `routes/mod.rs`.
pub const IMPORT_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

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

/// #288: validation failures from the memory store (`InvalidArgument`,
/// e.g. the content byte cap) are user errors — surface the message as a
/// 400 in the `{error}` envelope instead of a generic 500.
fn bad_request(msg: impl std::fmt::Display) -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": msg.to_string()})),
    )
}

fn internal(msg: impl std::fmt::Display) -> impl IntoResponse {
    // SEC-07: log detail server-side, return a generic 5xx so backend internals
    // don't leak to the client (mirrors the centralised `ApiError` mapping).
    tracing::error!(error = %msg, "memory endpoint internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "internal error"})),
    )
}

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListMemoriesQuery {
    pub kind: Option<String>,
    /// Comma-separated tag list (`tags=a,b`); a memory must carry every tag.
    /// One param instead of repeated `tags=` because plain
    /// `axum::extract::Query` cannot deserialize repeated keys into a `Vec`.
    pub tags: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

/// Split a comma-separated `tags=` param into trimmed, non-empty tags.
fn parse_tags_param(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub struct SimilarQuery {
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    5
}

// ─── Request bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateMemoryBody {
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

/// `GET /v1/memories?kind=&tags=&limit=&offset=`
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

    let tags = parse_tags_param(q.tags.as_deref());
    match store
        .list_memories(kind_filter, &tags, q.limit, q.offset)
        .await
    {
        Ok(memories) => Json(MemoryResponse { data: memories }).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `GET /v1/memories/:id`
pub async fn get_memory(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    match store.get_memory(id).await {
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
        kind,
        content: body.content,
        tags: body.tags,
        ttl_at: body.ttl_at,
    };

    match store.create_memory(req).await {
        Ok(m) => (StatusCode::CREATED, Json(MemoryResponse { data: m })).into_response(),
        // #288: content over MAX_CONTENT_BYTES → 400, not 500.
        Err(xiaoguai_memory::MemoryError::InvalidArgument(m)) => bad_request(m).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `PUT /v1/memories/:id`
pub async fn update_memory(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMemoryBody>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    let req = UpdateMemoryRequest {
        content: body.content,
        tags: body.tags,
        ttl_at: body.ttl_at,
    };

    match store.update_memory(id, req).await {
        Ok(m) => Json(MemoryResponse { data: m }).into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        // #288: content over MAX_CONTENT_BYTES → 400, not 500.
        Err(xiaoguai_memory::MemoryError::InvalidArgument(m)) => bad_request(m).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `DELETE /v1/memories/:id`
pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    match store.delete_memory(id).await {
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

// ─── Import / export (T7.2) ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub kind: Option<String>,
}

/// Best-effort `memory.*` audit entry through the generic `team_audit` sink
/// (the same one incidents use). Failure is logged, never blocks.
async fn audit_memory(state: &AppState, action: &str, details: serde_json::Value) {
    if let Some(sink) = &state.team_audit {
        let entry = xiaoguai_audit::AuditEntry {
            ts: Utc::now(),
            tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
            actor: "owner".to_string(),
            action: action.to_string(),
            resource: Some("memories".to_string()),
            details,
        };
        if let Err(e) = sink.append(entry).await {
            tracing::warn!(error = %e, action, "memory: audit append failed (non-blocking)");
        }
    }
}

/// `GET /v1/memories/export?kind=` — the whole store (or one kind) as a
/// `text/plain` JSONL document. Collected, not streamed: memory counts are
/// small by design. Embeddings are not exported (re-computed on import).
pub async fn export_memories(
    State(state): State<AppState>,
    Query(q): Query<ExportQuery>,
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

    match jsonl::export_jsonl_from_store(store.as_ref(), kind_filter).await {
        Ok(body) => {
            let count = body.lines().count();
            audit_memory(&state, "memory.export", serde_json::json!({"count": count})).await;
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                body,
            )
                .into_response()
        }
        Err(e) => internal(e).into_response(),
    }
}

/// `POST /v1/memories/import` — body is a `text/plain` JSONL document
/// (explicitly capped at [`IMPORT_BODY_LIMIT_BYTES`], #288).
/// Fail-soft per line (blank lines skipped silently, malformed lines
/// reported); each valid line re-embeds through the store's embedder; a
/// `source:imported` tag is added unless the line carries a `source:` tag.
/// Guardrails (#288): documents over `jsonl::MAX_IMPORT_LINES` raw lines
/// are rejected with 400; lines with an already-past `ttl_at` are skipped;
/// the run aborts early after `jsonl::MAX_CONSECUTIVE_STORE_FAILURES`
/// consecutive store failures (reported via `aborted`).
/// Response: `{imported: N, skipped: [{line, reason}], aborted?}`.
pub async fn import_memories(State(state): State<AppState>, body: String) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    match jsonl::import_jsonl(store.as_ref(), &body).await {
        Ok(report) => {
            audit_memory(
                &state,
                "memory.import",
                serde_json::json!({
                    "imported": report.imported,
                    "skipped": report.skipped.len(),
                    "aborted": report.aborted.is_some(),
                }),
            )
            .await;
            (StatusCode::OK, Json(report)).into_response()
        }
        // #288: line-cap pre-flight failure is a user error → 400.
        Err(xiaoguai_memory::MemoryError::InvalidArgument(m)) => bad_request(m).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `GET /v1/memories/similar/:id?top_k=`
pub async fn find_similar(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<SimilarQuery>,
) -> impl IntoResponse {
    let Some(store) = state.memory_store.as_ref() else {
        return memory_unavailable().into_response();
    };

    match store.find_similar(id, q.top_k).await {
        Ok(similar) => Json(MemoryResponse { data: similar }).into_response(),
        Err(xiaoguai_memory::MemoryError::NotFound(_)) => not_found(id).into_response(),
        Err(e) => internal(e).into_response(),
    }
}
