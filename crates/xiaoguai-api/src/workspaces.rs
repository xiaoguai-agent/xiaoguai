//! v1.3.x — Workspace concept (ADR-0019, Hermes inspiration).
//!
//! A workspace is an organisational grouping that sits above sessions/boards.
//! It lets operators partition work into named groups (teams, products,
//! projects).
//!
//! Trait lives in `xiaoguai-api` so handlers are storage-agnostic; the Pg
//! implementation lives in `xiaoguai-core/src/workspace_bridge.rs`.
//!
//! # REST surface
//!
//! | Method | Path                          | Description                        |
//! |--------|-------------------------------|------------------------------------|
//! | GET    | `/v1/workspaces`              | List workspaces                    |
//! | POST   | `/v1/workspaces`              | Create a new workspace             |
//! | PUT    | `/v1/workspaces/:id`          | Update name / archived flag        |
//! | DELETE | `/v1/workspaces/:id`          | Archive (soft-delete) a workspace  |
//!
//! When `workspace_id` is absent those routes default to the default
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
    pub name: String,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
}

/// Body accepted by `POST /v1/workspaces`.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateWorkspaceRequest {
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
    #[error("workspace name already exists")]
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
    /// Return all workspaces. Archived ones are included when
    /// `include_archived` is true; by default only active workspaces are
    /// returned.
    async fn list(&self, include_archived: bool) -> Result<Vec<Workspace>, WorkspaceError>;

    /// Create a new workspace. Returns `NameConflict` when a workspace with
    /// the same `name` already exists.
    async fn create(&self, req: CreateWorkspaceRequest) -> Result<Workspace, WorkspaceError>;

    /// Update `name` and/or `archived` flag. Returns `NotFound` when the id
    /// is unknown.
    async fn update(
        &self,
        id: Uuid,
        req: UpdateWorkspaceRequest,
    ) -> Result<Workspace, WorkspaceError>;

    /// Archive (soft-delete) a workspace. Returns `NotFound` when the id is
    /// unknown. The default workspace cannot be archived — callers get
    /// `InvalidArgument` if they try.
    async fn archive(&self, id: Uuid) -> Result<(), WorkspaceError>;

    /// Return the default workspace. Guaranteed to exist after migration
    /// 0017 has run.
    async fn get_default(&self) -> Result<Workspace, WorkspaceError>;
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

    /// Seed the default workspace — mirrors what migration 0017 does in
    /// production. Call this in test fixtures before exercises that rely on
    /// `get_default`.
    pub fn seed_default(&self) {
        let mut rows = self.rows.lock();
        let already = rows.iter().any(|w| w.name == "default");
        if already {
            return;
        }
        rows.push(Workspace {
            id: default_workspace_id(),
            name: "default".into(),
            archived: false,
            created_at: Utc::now(),
        });
    }
}

/// The deterministic default-workspace UUID. Stable within a process so
/// `get_default` round-trips against `seed_default`.
fn default_workspace_id() -> Uuid {
    // Namespace UUID for xiaoguai workspace seeds (arbitrary but fixed).
    let ns = Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        .expect("static namespace must parse");
    Uuid::new_v5(&ns, b"default")
}

#[async_trait]
impl WorkspaceRepository for InMemoryWorkspaceRepository {
    async fn list(&self, include_archived: bool) -> Result<Vec<Workspace>, WorkspaceError> {
        let rows = self.rows.lock();
        Ok(rows
            .iter()
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
        let conflict = rows.iter().any(|w| w.name == req.name);
        if conflict {
            return Err(WorkspaceError::NameConflict);
        }
        let workspace = Workspace {
            id: Uuid::new_v4(),
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

    async fn get_default(&self) -> Result<Workspace, WorkspaceError> {
        let rows = self.rows.lock();
        rows.iter()
            .find(|w| w.name == "default")
            .cloned()
            .ok_or(WorkspaceError::NotFound(default_workspace_id()))
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListWorkspacesQuery {
    #[serde(default)]
    pub include_archived: bool,
}

/// `GET /v1/workspaces[?include_archived=true]`
pub async fn list_workspaces(
    State(state): State<AppState>,
    Query(q): Query<ListWorkspacesQuery>,
) -> ApiResult<Json<Vec<Workspace>>> {
    let repo = workspace_repo(&state)?;
    let rows = repo
        .list(q.include_archived)
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

    #[tokio::test]
    async fn create_and_list_round_trip() {
        let repo = InMemoryWorkspaceRepository::new();
        let ws = repo
            .create(CreateWorkspaceRequest {
                name: "engineering".into(),
            })
            .await
            .unwrap();
        assert_eq!(ws.name, "engineering");
        assert!(!ws.archived);

        let list = repo.list(false).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, ws.id);
    }

    #[tokio::test]
    async fn duplicate_name_rejected() {
        let repo = InMemoryWorkspaceRepository::new();
        repo.create(CreateWorkspaceRequest {
            name: "ops".into(),
        })
        .await
        .unwrap();
        let err = repo
            .create(CreateWorkspaceRequest {
                name: "ops".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::NameConflict));
    }

    #[tokio::test]
    async fn archive_hides_from_default_list() {
        let repo = InMemoryWorkspaceRepository::new();
        let ws = repo
            .create(CreateWorkspaceRequest {
                name: "old-project".into(),
            })
            .await
            .unwrap();

        repo.archive(ws.id).await.unwrap();

        let active = repo.list(false).await.unwrap();
        assert!(active.is_empty());

        let all = repo.list(true).await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].archived);
    }

    #[tokio::test]
    async fn cannot_archive_default_workspace() {
        let repo = InMemoryWorkspaceRepository::new();
        repo.seed_default();

        let default = repo.get_default().await.unwrap();
        let err = repo.archive(default.id).await.unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_default_returns_seeded_workspace() {
        let repo = InMemoryWorkspaceRepository::new();
        repo.seed_default();

        let ws = repo.get_default().await.unwrap();
        assert_eq!(ws.name, "default");
    }

    #[tokio::test]
    async fn update_name_and_archived() {
        let repo = InMemoryWorkspaceRepository::new();
        let ws = repo
            .create(CreateWorkspaceRequest {
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
