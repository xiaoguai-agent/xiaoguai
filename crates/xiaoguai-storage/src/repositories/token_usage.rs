//! `TokenUsageRepository` — append-only ledger of per-call LLM token spend.
//!
//! All writes go through `record_batch` so the LLM router's background
//! flusher can amortise round-trips. Reads are ordered by timestamp
//! descending (most recent first). Single-owner deployment (DEC-033): no
//! tenant scoping — every read returns the whole ledger.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use crate::repositories::error::{RepoError, RepoResult};

#[derive(Debug, Clone)]
pub struct TokenUsageEntry {
    pub ts: DateTime<Utc>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub request_id: Option<String>,
}

#[derive(Debug, FromRow)]
struct TokenUsageRow {
    id: i64,
    ts: DateTime<Utc>,
    user_id: Option<String>,
    session_id: Option<String>,
    provider_id: String,
    model: String,
    prompt_tokens: Option<i32>,
    completion_tokens: Option<i32>,
    total_tokens: Option<i32>,
    request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredTokenUsage {
    pub id: i64,
    pub entry: TokenUsageEntry,
}

impl From<TokenUsageRow> for StoredTokenUsage {
    fn from(row: TokenUsageRow) -> Self {
        Self {
            id: row.id,
            entry: TokenUsageEntry {
                ts: row.ts,
                user_id: row.user_id,
                session_id: row.session_id,
                provider_id: row.provider_id,
                model: row.model,
                prompt_tokens: row.prompt_tokens,
                completion_tokens: row.completion_tokens,
                total_tokens: row.total_tokens,
                request_id: row.request_id,
            },
        }
    }
}

#[async_trait]
pub trait TokenUsageRepository: Send + Sync {
    /// Insert a batch of records in a single query. Empty input is a no-op.
    async fn record_batch(&self, entries: &[TokenUsageEntry]) -> RepoResult<()>;

    /// Return the most recent `limit` ledger entries, newest first.
    async fn list(&self, limit: i64) -> RepoResult<Vec<StoredTokenUsage>>;
}

#[derive(Debug, Clone)]
pub struct PgTokenUsageRepository {
    pool: SqlitePool,
}

impl PgTokenUsageRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TokenUsageRepository for PgTokenUsageRepository {
    async fn record_batch(&self, entries: &[TokenUsageEntry]) -> RepoResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "INSERT INTO token_usage \
             (ts, user_id, session_id, provider_id, model, \
              prompt_tokens, completion_tokens, total_tokens, request_id) ",
        );
        qb.push_values(entries.iter(), |mut b, e| {
            b.push_bind(e.ts)
                .push_bind(e.user_id.as_deref())
                .push_bind(e.session_id.as_deref())
                .push_bind(&e.provider_id)
                .push_bind(&e.model)
                .push_bind(e.prompt_tokens)
                .push_bind(e.completion_tokens)
                .push_bind(e.total_tokens)
                .push_bind(e.request_id.as_deref());
        });
        qb.build()
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn list(&self, limit: i64) -> RepoResult<Vec<StoredTokenUsage>> {
        if limit < 0 {
            return Err(RepoError::InvalidArgument(
                "limit must be non-negative".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let rows = sqlx::query_as::<_, TokenUsageRow>(
            "SELECT id, ts, user_id, session_id, provider_id, model, \
             prompt_tokens, completion_tokens, total_tokens, request_id \
             FROM token_usage \
             ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
