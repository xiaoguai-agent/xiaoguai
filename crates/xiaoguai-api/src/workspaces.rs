//! v1.3.x — Workspace concept (ADR-0019, Hermes inspiration).
//!
//! A workspace is an organisational grouping that sits above sessions/boards
//! but below tenant.  It lets operators partition work into named groups
//! (teams, products, projects) without needing a separate tenant.
//!
//! Trait lives in `xiaoguai-api` so handlers are storage-agnostic; the Pg
//! implementation lives in `xiaoguai-core/src/workspace_bridge.rs`.
//!
//! # REST surface
//!
//! | Method | Path                          | Description                        |
//! |--------|-------------------------------|------------------------------------|
//! | GET    | `/v1/workspaces`              | List workspaces for a tenant       |
//! | POST   | `/v1/workspaces`              | Create a new workspace             |
//! | PUT    | `/v1/workspaces/:id`          | Update name / archived flag        |
//! | DELETE | `/v1/workspaces/:id`          | Archive (soft-delete) a workspace  |
//!
//! Existing endpoints that accept a `?workspace_id=<uuid>` filter:
//!   - `GET /v1/sessions`
//!   - `GET /v1/outcomes/summary`
//!   - `GET /v1/outcomes/timeseries`
//!   - `GET /v1/skills/installed`
//!   - `GET /v1/hotl/policies`
//!
//! When `workspace_id` is absent those routes default to the tenant's default
//! workspace (the one seeded by migration 0017).

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One workspace row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
}

/// Body accepted by `POST /v1/workspaces`.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub tenant_id: Uuid,
    pub name: String,
}

/// Body accepted by `PUT /v1/workspaces/:id`.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateWorkspaceRequest {
    /// New name, if provided.
    pub name: Option<String>,
    /// Toggle the archived flag.
    pub archived: Option<bool>,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone)]
pub enum WorkspaceError {
    #[error("workspace not found: {0}")]
    NotFound(Uuid),
    #[error("workspace name already exists for this tenant")]
    NameConflict,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("backend: {0}")]
    Backend(String),
}

fn map_workspace_err(e: WorkspaceError) -> ApiError {
    match e {
        WorkspaceError::NotFound(_) => ApiError::NotFound,
        WorkspaceError::NameConflict => ApiError::Conflict("workspace name already exists".into()),
        WorkspaceError::InvalidArgument(msg) => ApiError::InvalidRequest(msg),
        WorkspaceError::Backend(msg) => ApiError::Internal(anyhow::anyhow!("workspace: {msg}")),
    }
}

// ---------------------------------------------------------------------------
// Repository trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait WorkspaceRepository: Send + Sync + std::fmt::Debug {
    /// Return all workspaces for `tenant_id`. Archived ones are included when
    /// `include_archived` is true; by default only active workspaces are
    /// returned.
    async fn list(
        &self,
        tenant_id: Uuid,
        include_archived: bool,
    ) -> Result<Vec<Workspace>, WorkspaceError>;

    /// Create a new workspace. Returns `NameConflict` when a workspace with
    /// the same `name` already exists for the tenant.
    async fn create(&self, req: CreateWorkspaceRequest) -> Result<Workspace, WorkspaceError>;

    /// Update `name` and/or `archived` flag. Returns `NotFound` when the id
    /// is unknown for the tenant.
    async fn update(
        &self,
        id: Uuid,
        req: UpdateWorkspaceRequest,
    ) -> Result<Workspace, WorkspaceError>;

    /// Archive (soft-delete) a workspace. Returns `NotFound` when the id is
    /// unknown. The default workspace cannot be archived — callers get
    /// `InvalidArgument` if they try.
    async fn archive(&self, id: Uuid) -> Result<(), WorkspaceError>;

    /// Return the default workspace for `tenant_id`. Guaranteed to exist after
    /// migration 0017 has run.
    async fn get_default(&self, tenant_id: Uuid) -> Result<Workspace, WorkspaceError>;
}

// ---------------------------------------------------------------------------
// In-memory implementation (tests / dev)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct InMemoryWorkspaceRepository {
    rows: Mutex<Vec<Workspace>>,
}

impl InMemoryWorkspaceRepository {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Seed a default workspace for `tenant_id` — mirrors what migration 0017
    /// does in production. Call this in test fixtures before exercises that
    /// rely on `get_default`.
    pub fn seed_default(&self, tenant_id: Uuid) {
        let mut rows = self.rows.lock();
        let already = rows
            .iter()
            .any(|w| w.tenant_id == tenant_id && w.name == "default");
        if already {
            return;
        }
        rows.push(Workspace {
            id: default_workspace_id(tenant_id),
            tenant_id,
            name: "default".into(),
            archived: false,
            created_at: Utc::now(),
        });
    }
}

/// Derive the deterministic default-workspace UUID the same way migration 0017
/// does: `CAST(md5(tenant_id || ':default') AS UUID)`.
///
/// We replicate the md5 byte layout without pulling in the `md5` crate by
/// using Uuid v5 (SHA-1-based namespace UUID). The in-memory impl only needs
/// this to be stable within a single test run, which v5 guarantees.
fn default_workspace_id(tenant_id: Uuid) -> Uuid {
    // Namespace UUID for xiaoguai workspace seeds (arbitrary but fixed).
    let ns = Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        .expect("static namespace must parse");
    Uuid::new_v5(&ns, format!("{tenant_id}:default").as_bytes())
}

#[async_trait]
impl WorkspaceRepository for InMemoryWorkspaceRepository {
    async fn list(
        &self,
        tenant_id: Uuid,
        include_archived: bool,
    ) -> Result<Vec<Workspace>, WorkspaceError> {
        let rows = self.rows.lock();
        Ok(rows
            .iter()
            .filter(|w| w.tenant_id == tenant_id)
            .filter(|w| include_archived || !w.archived)
            .cloned()
            .collect())
    }

    async fn create(&self, req: CreateWorkspaceRequest) -> Result<Workspace, WorkspaceError> {
        if req.name.trim().is_empty() {
            return Err(WorkspaceError::InvalidArgument(
                "workspace name must not be empty".into(),
            ));
        }
        let mut rows = self.rows.lock();
        let conflict = rows
            .iter()
            .any(|w| w.tenant_id == req.tenant_id && w.name == req.name);
        if conflict {
            return Err(WorkspaceError::NameConflict);
        }
        let workspace = Workspace {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            name: req.name,
            archived: false,
            created_at: Utc::now(),
        };
        rows.push(workspace.clone());
        Ok(workspace)
    }

    async fn update(
        &self,
        id: Uuid,
        req: UpdateWorkspaceRequest,
    ) -> Result<Workspace, WorkspaceError> {
        let mut rows = self.rows.lock();
        let pos = rows
            .iter()
            .position(|w| w.id == id)
            .ok_or(WorkspaceError::NotFound(id))?;
        let existing = rows[pos].clone();
        let updated = Workspace {
            name: req.name.unwrap_or(existing.name),
            archived: req.archived.unwrap_or(existing.archived),
            ..existing
        };
        rows[pos] = updated.clone();
        Ok(updated)
    }

    async fn archive(&self, id: Uuid) -> Result<(), WorkspaceError> {
        let mut rows = self.rows.lock();
        let pos = rows
            .iter()
            .position(|w| w.id == id)
            .ok_or(WorkspaceError::NotFound(id))?;
        let existing = &rows[pos];
        if existing.name == "default" {
            return Err(WorkspaceError::InvalidArgument(
                "the default workspace cannot be archived".into(),
            ));
        }
        let updated = Workspace {
            archived: true,
            ..existing.clone()
        };
        rows[pos] = updated;
        Ok(())
    }

    async fn get_default(&self, tenant_id: Uuid) -> Result<Workspace, WorkspaceError> {
        let rows = self.rows.lock();
        rows.iter()
            .find(|w| w.tenant_id == tenant_id && w.name == "default")
            .cloned()
            .ok_or(WorkspaceError::NotFound(tenant_id))
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListWorkspacesQuery {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub include_archived: bool,
}

/// `GET /v1/workspaces?tenant_id=<uuid>[&include_archived=true]`
pub async fn list_workspaces(
    State(state): State<AppState>,
    Query(q): Query<ListWorkspacesQuery>,
) -> ApiResult<Json<Vec<Workspace>>> {
    let repo = workspace_repo(&state)?;
    let rows = repo
        .list(q.tenant_id, q.include_archived)
        .await
        .map_err(map_workspace_err)?;
    Ok(Json(rows))
}

/// `POST /v1/workspaces`
pub async fn create_workspace(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<(StatusCode, Json<Workspace>)> {
    let repo = workspace_repo(&state)?;
    let workspace = repo.create(req).await.map_err(map_workspace_err)?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

/// `PUT /v1/workspaces/:id`
pub async fn update_workspace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> ApiResult<Json<Workspace>> {
    let repo = workspace_repo(&state)?;
    let workspace = repo.update(id, req).await.map_err(map_workspace_err)?;
    Ok(Json(workspace))
}

/// `DELETE /v1/workspaces/:id`
///
/// Soft-deletes (archives) the workspace. Returns `204 No Content`.
pub async fn archive_workspace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = workspace_repo(&state)?;
    repo.archive(id).await.map_err(map_workspace_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn workspace_repo(state: &AppState) -> ApiResult<Arc<dyn WorkspaceRepository>> {
    state
        .workspace_repository
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("workspace repository not wired".into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tid() -> Uuid {
        Uuid::new_v4()
    }

    #[tokio::test]
    async fn create_and_list_round_trip() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        let ws = repo
            .create(CreateWorkspaceRequest {
                tenant_id: tenant,
                name: "engineering".into(),
            })
            .await
            .unwrap();
        assert_eq!(ws.name, "engineering");
        assert!(!ws.archived);

        let list = repo.list(tenant, false).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, ws.id);
    }

    #[tokio::test]
    async fn duplicate_name_rejected() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        repo.create(CreateWorkspaceRequest {
            tenant_id: tenant,
            name: "ops".into(),
        })
        .await
        .unwrap();
        let err = repo
            .create(CreateWorkspaceRequest {
                tenant_id: tenant,
                name: "ops".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::NameConflict));
    }

    #[tokio::test]
    async fn list_scopes_by_tenant() {
        let repo = InMemoryWorkspaceRepository::new();
        let t1 = tid();
        let t2 = tid();
        repo.create(CreateWorkspaceRequest {
            tenant_id: t1,
            name: "alpha".into(),
        })
        .await
        .unwrap();
        repo.create(CreateWorkspaceRequest {
            tenant_id: t2,
            name: "beta".into(),
        })
        .await
        .unwrap();

        assert_eq!(repo.list(t1, false).await.unwrap().len(), 1);
        assert_eq!(repo.list(t2, false).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn archive_hides_from_default_list() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        let ws = repo
            .create(CreateWorkspaceRequest {
                tenant_id: tenant,
                name: "old-project".into(),
            })
            .await
            .unwrap();

        repo.archive(ws.id).await.unwrap();

        let active = repo.list(tenant, false).await.unwrap();
        assert!(active.is_empty());

        let all = repo.list(tenant, true).await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].archived);
    }

    #[tokio::test]
    async fn cannot_archive_default_workspace() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        repo.seed_default(tenant);

        let default = repo.get_default(tenant).await.unwrap();
        let err = repo.archive(default.id).await.unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_default_returns_seeded_workspace() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        repo.seed_default(tenant);

        let ws = repo.get_default(tenant).await.unwrap();
        assert_eq!(ws.name, "default");
        assert_eq!(ws.tenant_id, tenant);
    }

    #[tokio::test]
    async fn update_name_and_archived() {
        let repo = InMemoryWorkspaceRepository::new();
        let tenant = tid();
        let ws = repo
            .create(CreateWorkspaceRequest {
                tenant_id: tenant,
                name: "original".into(),
            })
            .await
            .unwrap();

        let updated = repo
            .update(
                ws.id,
                UpdateWorkspaceRequest {
                    name: Some("renamed".into()),
                    archived: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "renamed");
        assert!(!updated.archived);
    }

    #[tokio::test]
    async fn update_unknown_id_is_not_found() {
        let repo = InMemoryWorkspaceRepository::new();
        let err = repo
            .update(
                Uuid::new_v4(),
                UpdateWorkspaceRequest {
                    name: None,
                    archived: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::NotFound(_)));
    }

    #[tokio::test]
    async fn empty_name_rejected() {
        let repo = InMemoryWorkspaceRepository::new();
        let err = repo
            .create(CreateWorkspaceRequest {
                tenant_id: tid(),
                name: "  ".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn archive_unknown_id_is_not_found() {
        let repo = InMemoryWorkspaceRepository::new();
        let err = repo.archive(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, WorkspaceError::NotFound(_)));
    }
}
