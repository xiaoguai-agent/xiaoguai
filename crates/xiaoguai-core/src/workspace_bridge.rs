//! Postgres implementation of [`WorkspaceRepository`] (v1.3.x).
//!
//! Reads and writes the `workspaces` table introduced by migration 0017.
//! Follows the same bridge pattern as [`crate::sessions_bridge`] and
//! [`crate::usage_bridge`]: thin `sqlx` queries, typed conversions, no
//! business logic.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use xiaoguai_api::workspaces::{
    CreateWorkspaceRequest, UpdateWorkspaceRequest, Workspace, WorkspaceError, WorkspaceRepository,
};

// ---------------------------------------------------------------------------
// Row type (matches the `workspaces` table exactly)
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    archived: bool,
    created_at: DateTime<Utc>,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            name: r.name,
            archived: r.archived,
            created_at: r.created_at,
        }
    }
}

// ---------------------------------------------------------------------------
// PgWorkspaceRepository
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PgWorkspaceRepository {
    pool: PgPool,
}

impl PgWorkspaceRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<Self> {
        Arc::new(Self::new(pool))
    }
}

#[async_trait]
impl WorkspaceRepository for PgWorkspaceRepository {
    async fn list(
        &self,
        tenant_id: Uuid,
        include_archived: bool,
    ) -> Result<Vec<Workspace>, WorkspaceError> {
        let rows = if include_archived {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, tenant_id, name, archived, created_at
                 FROM workspaces
                 WHERE tenant_id = $1
                 ORDER BY created_at ASC",
            )
            .bind(tenant_id)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, tenant_id, name, archived, created_at
                 FROM workspaces
                 WHERE tenant_id = $1
                   AND archived = FALSE
                 ORDER BY created_at ASC",
            )
            .bind(tenant_id)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?;

        Ok(rows.into_iter().map(Workspace::from).collect())
    }

    async fn create(&self, req: CreateWorkspaceRequest) -> Result<Workspace, WorkspaceError> {
        if req.name.trim().is_empty() {
            return Err(WorkspaceError::InvalidArgument(
                "workspace name must not be empty".into(),
            ));
        }

        let row = sqlx::query_as::<_, WorkspaceRow>(
            "INSERT INTO workspaces (tenant_id, name)
             VALUES ($1, $2)
             RETURNING id, tenant_id, name, archived, created_at",
        )
        .bind(req.tenant_id)
        .bind(&req.name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate") {
                WorkspaceError::NameConflict
            } else {
                WorkspaceError::Backend(msg)
            }
        })?;

        Ok(Workspace::from(row))
    }

    async fn update(
        &self,
        id: Uuid,
        req: UpdateWorkspaceRequest,
    ) -> Result<Workspace, WorkspaceError> {
        // Build a dynamic UPDATE: only touch columns that have a new value.
        // We read the current row first so a partial update doesn't clobber
        // unset fields.
        let current = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, tenant_id, name, archived, created_at
             FROM workspaces WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?
        .ok_or(WorkspaceError::NotFound(id))?;

        let new_name = req.name.unwrap_or(current.name);
        let new_archived = req.archived.unwrap_or(current.archived);

        let row = sqlx::query_as::<_, WorkspaceRow>(
            "UPDATE workspaces
             SET name     = $2,
                 archived = $3
             WHERE id = $1
             RETURNING id, tenant_id, name, archived, created_at",
        )
        .bind(id)
        .bind(&new_name)
        .bind(new_archived)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate") {
                WorkspaceError::NameConflict
            } else {
                WorkspaceError::Backend(msg)
            }
        })?
        .ok_or(WorkspaceError::NotFound(id))?;

        Ok(Workspace::from(row))
    }

    async fn archive(&self, id: Uuid) -> Result<(), WorkspaceError> {
        // Prevent archiving the default workspace.
        let current = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, tenant_id, name, archived, created_at
             FROM workspaces WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?
        .ok_or(WorkspaceError::NotFound(id))?;

        if current.name == "default" {
            return Err(WorkspaceError::InvalidArgument(
                "the default workspace cannot be archived".into(),
            ));
        }

        let affected = sqlx::query("UPDATE workspaces SET archived = TRUE WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| WorkspaceError::Backend(e.to_string()))?
            .rows_affected();

        if affected == 0 {
            return Err(WorkspaceError::NotFound(id));
        }
        Ok(())
    }

    async fn get_default(&self, tenant_id: Uuid) -> Result<Workspace, WorkspaceError> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, tenant_id, name, archived, created_at
             FROM workspaces
             WHERE tenant_id = $1
               AND name = 'default'
             LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?
        .ok_or(WorkspaceError::NotFound(tenant_id))?;

        Ok(Workspace::from(row))
    }
}
