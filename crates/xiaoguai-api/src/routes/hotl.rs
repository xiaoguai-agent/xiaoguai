//! `/v1/hotl/policies` — HOTL boundary policy admin endpoints (v1.2.3).
//!
//! All three routes are bearer-gated (via the outer v1 middleware stack).
//!
//! | Method | Path                         | Description                  |
//! |--------|------------------------------|------------------------------|
//! | GET    | `/v1/hotl/policies`          | List policies for a tenant   |
//! | POST   | `/v1/hotl/policies`          | Create a new policy          |
//! | DELETE | `/v1/hotl/policies/:id`      | Remove a policy by id        |

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::hotl::policy::{CreateHotlPolicyRequest, HotlPolicy, HotlPolicyStoreError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListPoliciesQuery {
    pub tenant_id: Uuid,
    pub scope: Option<String>,
}

/// `GET /v1/hotl/policies?tenant_id=<uuid>[&scope=<str>]`
///
/// Returns all HOTL policies for the given tenant, optionally filtered by
/// `scope`. Returns 503 when no store is wired into `AppState`.
///
/// # Errors
/// Returns an error if the policy store is not wired or the query fails.
pub async fn list_policies(
    State(state): State<AppState>,
    Query(q): Query<ListPoliciesQuery>,
) -> ApiResult<Json<Vec<HotlPolicy>>> {
    let store = state
        .hotl_policy_store
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("HOTL policy store not wired".into()))?;
    let rows = store
        .list(q.tenant_id, q.scope.as_deref())
        .await
        .map_err(map_store_err)?;
    Ok(Json(rows))
}

/// `POST /v1/hotl/policies`
///
/// Body: [`CreateHotlPolicyRequest`].
/// Returns `201 Created` with the persisted [`HotlPolicy`].
///
/// # Errors
/// Returns an error if the policy store is not wired or the request is invalid.
pub async fn create_policy(
    State(state): State<AppState>,
    Json(req): Json<CreateHotlPolicyRequest>,
) -> ApiResult<(StatusCode, Json<HotlPolicy>)> {
    let store = state
        .hotl_policy_store
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("HOTL policy store not wired".into()))?;
    let policy = store.create(req).await.map_err(map_store_err)?;
    Ok((StatusCode::CREATED, Json(policy)))
}

/// `DELETE /v1/hotl/policies/:id`
///
/// Returns `204 No Content` on success; `404 Not Found` when the id is
/// unknown.
///
/// # Errors
/// Returns an error if the policy store is not wired or the policy is not found.
pub async fn delete_policy(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let store = state
        .hotl_policy_store
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("HOTL policy store not wired".into()))?;
    store.delete(id).await.map_err(map_store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn map_store_err(e: HotlPolicyStoreError) -> ApiError {
    match e {
        HotlPolicyStoreError::NotFound(_) => ApiError::NotFound,
        HotlPolicyStoreError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        HotlPolicyStoreError::Backend(msg) => {
            ApiError::Internal(anyhow::anyhow!("HOTL store: {msg}"))
        }
    }
}
