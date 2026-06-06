//! `MessageRepository` — `SQLite` implementation backed by sqlx.
//!
//! Single-owner deployment (DEC-033): messages are not tenant-scoped and every
//! method runs in a plain transaction. The `content` column carries a
//! serialized `Vec<ContentBlock>` (using `serde(tag = "type")` discrimination).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{types::Json, FromRow, SqlitePool};
use xiaoguai_types::{ContentBlock, Message, MessageId, MessageRole, SessionId};

use crate::repositories::error::{RepoError, RepoResult};

#[async_trait]
pub trait MessageRepository: Send + Sync {
    async fn append(&self, message: &Message) -> RepoResult<()>;
    async fn list_by_session(
        &self,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Message>>;
    async fn count_by_session(&self, session_id: &str) -> RepoResult<i64>;
    /// Bulk-delete a session's messages. NOTE: currently unreachable — no
    /// route or internal caller exposes it, so it is not wired into the HMAC
    /// audit chain. If you ever expose it, emit a `message.delete` audit entry
    /// at the call site (see the `agent.run` entry in `routes/sessions.rs`).
    async fn delete_by_session(&self, session_id: &str) -> RepoResult<u64>;
}

#[derive(Debug, Clone)]
pub struct SqliteMessageRepository {
    pool: SqlitePool,
}

impl SqliteMessageRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct MessageRow {
    id: String,
    session_id: String,
    role: String,
    content: Json<Vec<ContentBlock>>,
    created_at: DateTime<Utc>,
}

impl MessageRow {
    fn into_domain(self) -> RepoResult<Message> {
        let role = match self.role.as_str() {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            other => {
                return Err(RepoError::InvalidArgument(format!(
                    "unknown message role: {other}"
                )));
            }
        };
        Ok(Message {
            id: MessageId::from(self.id),
            session_id: SessionId::from(self.session_id),
            role,
            content: self.content.0,
            created_at: self.created_at,
        })
    }
}

fn role_str(r: MessageRole) -> &'static str {
    match r {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

#[async_trait]
impl MessageRepository for SqliteMessageRepository {
    async fn append(&self, message: &Message) -> RepoResult<()> {
        let content = Json(&message.content);
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(message.id.as_str())
        .bind(message.session_id.as_str())
        .bind(role_str(message.role))
        .bind(content)
        .bind(message.created_at)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn list_by_session(
        &self,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Message>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit/offset must be >= 0".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT id, session_id, role, content, created_at
             FROM messages
             WHERE session_id = ?
             ORDER BY created_at ASC, id ASC
             LIMIT ? OFFSET ?",
        )
        .bind(session_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(MessageRow::into_domain).collect()
    }

    async fn count_by_session(&self, session_id: &str) -> RepoResult<i64> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM messages WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(count)
    }

    async fn delete_by_session(&self, session_id: &str) -> RepoResult<u64> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let result = sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected())
    }
}
