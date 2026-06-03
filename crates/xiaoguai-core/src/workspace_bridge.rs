//! `SQLite` implementation of [`WorkspaceRepository`] (v1.3.x).
//!
//! Reads and writes the `workspaces` table introduced by migration 0017.
//! Follows the same bridge pattern as [`crate::sessions_bridge`] and
//! [`crate::usage_bridge`]: thin `sqlx` queries, typed conversions, no
//! business logic.
//!
//! DEC-033 single-user pivot: the `workspaces.tenant_id` column was dropped
//! (one implicit owner). The domain [`Workspace`] type still carries a
//! `tenant_id: Uuid` field, so reads synthesize `Uuid::nil()`. The `id`
//! column is now `TEXT` with no DB-side default, so `create` generates a
//! v4 UUID app-side. The UNIQUE constraint is now just `(name)`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;
use xiaoguai_api::workspaces::{
    CreateWorkspaceRequest, UpdateWorkspaceRequest, Workspace, WorkspaceError, WorkspaceRepository,
};

// ---------------------------------------------------------------------------
// Row type (matches the `workspaces` table exactly — no tenant_id)
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    archived: bool,
    created_at: DateTime<Utc>,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Self {
            id: r.id,
            // Single implicit owner under DEC-033; no per-tenant scoping.
            tenant_id: Uuid::nil(),
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
    pool: SqlitePool,
}

impl PgWorkspaceRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self::new(pool))
    }
}

#[async_trait]
impl WorkspaceRepository for PgWorkspaceRepository {
    async fn list(
        &self,
        _tenant_id: Uuid,
        include_archived: bool,
    ) -> Result<Vec<Workspace>, WorkspaceError> {
        let rows = if include_archived {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, name, archived, created_at
                 FROM workspaces
                 ORDER BY created_at ASC",
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, name, archived, created_at
                 FROM workspaces
                 WHERE archived = FALSE
                 ORDER BY created_at ASC",
            )
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

        // `id` has no DB-side default under SQLite — generate app-side.
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "INSERT INTO workspaces (id, name)
             VALUES (?, ?)
             RETURNING id, name, archived, created_at",
        )
        .bind(id)
        .bind(&req.name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate") || msg.contains("UNIQUE") {
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
            "SELECT id, name, archived, created_at
             FROM workspaces WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?
        .ok_or(WorkspaceError::NotFound(id))?;

        let new_name = req.name.unwrap_or(current.name);
        let new_archived = req.archived.unwrap_or(current.archived);

        // `id` is referenced ($1) after name/archived ($2/$3) — use numbered
        // binds so the bind order stays correct.
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "UPDATE workspaces
             SET name     = ?2,
                 archived = ?3
             WHERE id = ?1
             RETURNING id, name, archived, created_at",
        )
        .bind(id)
        .bind(&new_name)
        .bind(new_archived)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate") || msg.contains("UNIQUE") {
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
            "SELECT id, name, archived, created_at
             FROM workspaces WHERE id = ?",
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

        let affected = sqlx::query("UPDATE workspaces SET archived = TRUE WHERE id = ?")
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

    async fn get_default(&self, _tenant_id: Uuid) -> Result<Workspace, WorkspaceError> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, name, archived, created_at
             FROM workspaces
             WHERE name = 'default'
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WorkspaceError::Backend(e.to_string()))?
        .ok_or(WorkspaceError::NotFound(Uuid::nil()))?;

        Ok(Workspace::from(row))
    }
}
