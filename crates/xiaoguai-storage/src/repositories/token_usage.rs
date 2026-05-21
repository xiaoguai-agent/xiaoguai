//! `TokenUsageRepository` — append-only ledger of per-call LLM token spend.
//!
//! All writes go through `record_batch` so the LLM router's background
//! flusher can amortise round-trips. Reads are tenant-scoped and ordered by
//! timestamp descending (most recent first) — typical "what did this tenant
//! cost in the last hour?" queries.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};

use crate::repositories::error::{RepoError, RepoResult};

#[derive(Debug, Clone)]
pub struct TokenUsageEntry {
    pub ts: DateTime<Utc>,
    pub tenant_id: String,
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
    tenant_id: String,
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
                tenant_id: row.tenant_id,
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

    async fn list_for_tenant(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> RepoResult<Vec<StoredTokenUsage>>;
}

#[derive(Debug, Clone)]
pub struct PgTokenUsageRepository {
    pool: PgPool,
}

impl PgTokenUsageRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TokenUsageRepository for PgTokenUsageRepository {
    async fn record_batch(&self, entries: &[TokenUsageEntry]) -> RepoResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO token_usage \
             (ts, tenant_id, user_id, session_id, provider_id, model, \
              prompt_tokens, completion_tokens, total_tokens, request_id) ",
        );
        qb.push_values(entries.iter(), |mut b, e| {
            b.push_bind(e.ts)
                .push_bind(&e.tenant_id)
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
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn list_for_tenant(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> RepoResult<Vec<StoredTokenUsage>> {
        if limit < 0 {
            return Err(RepoError::InvalidArgument(
                "limit must be non-negative".into(),
            ));
        }
        let rows = sqlx::query_as::<_, TokenUsageRow>(
            "SELECT id, ts, tenant_id, user_id, session_id, provider_id, model, \
             prompt_tokens, completion_tokens, total_tokens, request_id \
             FROM token_usage WHERE tenant_id = $1 \
             ORDER BY ts DESC LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
