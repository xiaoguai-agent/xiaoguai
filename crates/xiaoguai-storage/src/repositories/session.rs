//! `SessionRepository` — Postgres implementation backed by sqlx.
//!
//! Session rows are tenant-scoped; RLS policies in `0001_initial.sql` enforce
//! `tenant_id = current_setting('app.current_tenant_id')`. Callers are expected
//! to `SET LOCAL app.current_tenant_id` per transaction. The superuser used in
//! testcontainers bypasses non-FORCE RLS — production deployments should run
//! the app as a non-superuser role.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use xiaoguai_types::{Session, SessionId, SessionStatus, TenantId, UserId};

use crate::repositories::error::{RepoError, RepoResult};

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, session: &Session) -> RepoResult<()>;
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Session>>;
    async fn list_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>>;
    async fn touch(&self, id: &str) -> RepoResult<()>;
    async fn archive(&self, id: &str) -> RepoResult<()>;
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct PgSessionRepository {
    pool: PgPool,
}

impl PgSessionRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct SessionRow {
    id: String,
    tenant_id: String,
    user_id: String,
    title: Option<String>,
    model: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl SessionRow {
    fn into_domain(self) -> RepoResult<Session> {
        let status = match self.status.as_str() {
            "active" => SessionStatus::Active,
            "archived" => SessionStatus::Archived,
            other => {
                return Err(RepoError::InvalidArgument(format!(
                    "unknown session status: {other}"
                )));
            }
        };
        Ok(Session {
            id: SessionId::from(self.id),
            tenant_id: TenantId::from(self.tenant_id),
            user_id: UserId::from(self.user_id),
            title: self.title,
            created_at: self.created_at,
            updated_at: self.updated_at,
            model: self.model,
            status,
        })
    }
}

fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Active => "active",
        SessionStatus::Archived => "archived",
    }
}

#[async_trait]
impl SessionRepository for PgSessionRepository {
    async fn create(&self, session: &Session) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, tenant_id, user_id, title, model, status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(session.id.as_str())
        .bind(session.tenant_id.as_str())
        .bind(session.user_id.as_str())
        .bind(session.title.as_deref())
        .bind(&session.model)
        .bind(status_str(session.status))
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Session>> {
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT id, tenant_id, user_id, title, model, status, created_at, updated_at
             FROM sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        row.map(SessionRow::into_domain).transpose()
    }

    async fn list_by_user(
        &self,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit/offset must be >= 0".to_string(),
            ));
        }
        let rows: Vec<SessionRow> = sqlx::query_as(
            "SELECT id, tenant_id, user_id, title, model, status, created_at, updated_at
             FROM sessions
             WHERE user_id = $1
             ORDER BY updated_at DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(SessionRow::into_domain).collect()
    }

    async fn touch(&self, id: &str) -> RepoResult<()> {
        let result = sqlx::query("UPDATE sessions SET updated_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }

    async fn archive(&self, id: &str) -> RepoResult<()> {
        let result = sqlx::query(
            "UPDATE sessions SET status = 'archived', updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        // Idempotent — deleting a non-existent row is not an error. FK CASCADE
        // wipes child messages.
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
