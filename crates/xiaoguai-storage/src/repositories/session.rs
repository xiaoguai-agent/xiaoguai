//! `SessionRepository` — Postgres implementation backed by sqlx.
//!
//! Session rows are tenant-scoped; RLS policy `tenant_isolation_sessions` in
//! `0001_initial.sql` enforces
//! `tenant_id = current_setting('app.current_tenant_id')`.
//!
//! Every method takes a `tenant: Option<&str>` argument and runs inside a
//! transaction that sets the `app.current_tenant_id` GUC via
//! [`begin_tenant_tx`]. When `tenant` is `None` (admin / cross-tenant CLI
//! paths) no GUC is set; under a non-superuser DB role that means RLS
//! returns an empty result for the policy-protected columns. Tests use the
//! testcontainers `postgres` superuser, which bypasses non-FORCE RLS, so
//! `None` works there as a "see everything" mode.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use xiaoguai_types::{MessageId, Session, SessionId, SessionStatus, TenantId, UserId};

use crate::repositories::error::{RepoError, RepoResult};
use crate::repositories::tenant_ctx::begin_tenant_tx;

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, tenant: Option<&str>, session: &Session) -> RepoResult<()>;
    async fn find_by_id(&self, tenant: Option<&str>, id: &str) -> RepoResult<Option<Session>>;
    async fn list_by_user(
        &self,
        tenant: Option<&str>,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>>;
    async fn touch(&self, tenant: Option<&str>, id: &str) -> RepoResult<()>;
    async fn archive(&self, tenant: Option<&str>, id: &str) -> RepoResult<()>;
    async fn delete(&self, tenant: Option<&str>, id: &str) -> RepoResult<()>;

    /// v1.1.2 — clone a session and copy every message with
    /// `created_at <= cutoff.created_at` (where `cutoff` is the message
    /// identified by `from_message_id`) from the parent into the new
    /// session. The new session's `parent_session_id` /
    /// `forked_from_message_id` must already be set on `new_session`
    /// by the caller. Atomic: the new session row and the copied
    /// messages succeed or fail together.
    ///
    /// Default impl returns `Unsupported` so test mocks that don't care
    /// about forking compile unchanged; only the production
    /// `PgSessionRepository` overrides this.
    async fn fork(
        &self,
        _tenant: Option<&str>,
        _parent_id: &str,
        _from_message_id: &str,
        _new_session: &Session,
    ) -> RepoResult<()> {
        Err(RepoError::Unsupported(
            "SessionRepository::fork not implemented for this backend".into(),
        ))
    }
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
    // v1.1.2: fork lineage. Both nullable; only forked rows have them.
    parent_session_id: Option<String>,
    forked_from_message_id: Option<String>,
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
            parent_session_id: self.parent_session_id.map(SessionId::from),
            forked_from_message_id: self.forked_from_message_id.map(MessageId::from),
        })
    }
}

const SESSION_COLUMNS: &str =
    "id, tenant_id, user_id, title, model, status, created_at, updated_at, \
                               parent_session_id, forked_from_message_id";

fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Active => "active",
        SessionStatus::Archived => "archived",
    }
}

#[async_trait]
impl SessionRepository for PgSessionRepository {
    async fn create(&self, tenant: Option<&str>, session: &Session) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        sqlx::query(
            "INSERT INTO sessions (id, tenant_id, user_id, title, model, status, created_at, updated_at, parent_session_id, forked_from_message_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(session.id.as_str())
        .bind(session.tenant_id.as_str())
        .bind(session.user_id.as_str())
        .bind(session.title.as_deref())
        .bind(&session.model)
        .bind(status_str(session.status))
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(session.parent_session_id.as_ref().map(SessionId::as_str))
        .bind(session.forked_from_message_id.as_ref().map(MessageId::as_str))
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, tenant: Option<&str>, id: &str) -> RepoResult<Option<Session>> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        let row: Option<SessionRow> = sqlx::query_as(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        row.map(SessionRow::into_domain).transpose()
    }

    async fn list_by_user(
        &self,
        tenant: Option<&str>,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit/offset must be >= 0".to_string(),
            ));
        }
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        let rows: Vec<SessionRow> = sqlx::query_as(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions
             WHERE user_id = $1
             ORDER BY updated_at DESC
             LIMIT $2 OFFSET $3"
        ))
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(SessionRow::into_domain).collect()
    }

    async fn touch(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        let result = sqlx::query("UPDATE sessions SET updated_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn archive(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        let result = sqlx::query(
            "UPDATE sessions SET status = 'archived', updated_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn delete(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        // Idempotent — deleting a non-existent row is not an error. FK CASCADE
        // wipes child messages.
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    /// v1.1.2 — atomic fork. All work happens in one tenant-scoped tx:
    /// (1) look up the cutoff message inside the parent (and verify the
    /// parent exists), (2) insert the new session row, (3) copy every
    /// message of the parent with `created_at <= cutoff.created_at`
    /// into the child. Ordering by `created_at` deliberately: some seed
    /// IDs in tests are UUIDs and don't sort lexicographically the same
    /// way they were inserted. The new message rows get fresh IDs so
    /// the (`id` PK) constraint is preserved.
    async fn fork(
        &self,
        tenant: Option<&str>,
        parent_id: &str,
        from_message_id: &str,
        new_session: &Session,
    ) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;

        // (1) Verify the cutoff message belongs to the parent session.
        // The session/message FK + tenant RLS together guarantee tenant
        // isolation here — a caller asking us to fork another tenant's
        // session sees `NotFound` rather than a leaked row.
        let cutoff_ts: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT created_at FROM messages WHERE id = $1 AND session_id = $2")
                .bind(from_message_id)
                .bind(parent_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(RepoError::from_sqlx)?;
        let cutoff_ts = cutoff_ts.ok_or(RepoError::NotFound)?;

        // (2) Insert the new session row.
        sqlx::query(
            "INSERT INTO sessions (id, tenant_id, user_id, title, model, status, created_at, updated_at, parent_session_id, forked_from_message_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(new_session.id.as_str())
        .bind(new_session.tenant_id.as_str())
        .bind(new_session.user_id.as_str())
        .bind(new_session.title.as_deref())
        .bind(&new_session.model)
        .bind(status_str(new_session.status))
        .bind(new_session.created_at)
        .bind(new_session.updated_at)
        .bind(new_session.parent_session_id.as_ref().map(SessionId::as_str))
        .bind(new_session.forked_from_message_id.as_ref().map(MessageId::as_str))
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        // (3) Copy the prefix. Fresh `id` per row via gen_random_uuid()
        // prefixed so it's visually distinct from app-generated ids.
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             SELECT 'msg_' || gen_random_uuid()::text, $1, role, content, created_at
             FROM messages
             WHERE session_id = $2 AND created_at <= $3
             ORDER BY created_at ASC, id ASC",
        )
        .bind(new_session.id.as_str())
        .bind(parent_id)
        .bind(cutoff_ts)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
