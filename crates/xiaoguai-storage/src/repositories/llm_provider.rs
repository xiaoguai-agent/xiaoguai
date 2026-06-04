//! `LlmProviderRepository` — `SQLite`-backed registry of LLM providers.
//!
//! Single-owner deployment (DEC-033): all providers belong to the one owner;
//! every method runs in a plain transaction.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use xiaoguai_types::{ids::ProviderId, LlmProvider, ProviderKind};

use crate::repositories::error::{RepoError, RepoResult};

#[async_trait]
pub trait LlmProviderRepository: Send + Sync {
    async fn create(&self, prov: &LlmProvider) -> RepoResult<()>;
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>>;
    /// All registered providers, ordered by `fallback_order` ascending.
    async fn list(&self) -> RepoResult<Vec<LlmProvider>>;
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct PgLlmProviderRepository {
    pool: SqlitePool,
}

impl PgLlmProviderRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct LlmProviderRow {
    id: String,
    name: String,
    kind: String,
    endpoint: String,
    models: serde_json::Value,
    default_for_models: serde_json::Value,
    fallback_order: i32,
    api_key_env: Option<String>,
    /// Directly-stored API key (web-UI providers); NULL for env-var /
    /// unauthenticated providers. Added in migration 0029.
    #[sqlx(default)]
    api_key: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// v1.1.1.1 — may be NULL if the column was added after the row was
    /// inserted (pre-migration rows) or for mock/test providers.
    cost_per_1k_input_usd: Option<f64>,
    cost_per_1k_output_usd: Option<f64>,
}

impl LlmProviderRow {
    fn into_domain(self) -> RepoResult<LlmProvider> {
        let kind = ProviderKind::parse(&self.kind).ok_or_else(|| {
            RepoError::InvalidArgument(format!("unknown provider kind in DB: {}", self.kind))
        })?;
        let models: Vec<String> = serde_json::from_value(self.models)?;
        let default_for_models: Vec<String> = serde_json::from_value(self.default_for_models)?;
        Ok(LlmProvider {
            id: ProviderId::from(self.id),
            name: self.name,
            kind,
            endpoint: self.endpoint,
            models,
            default_for_models,
            fallback_order: self.fallback_order,
            api_key_env: self.api_key_env,
            api_key: self.api_key,
            created_at: self.created_at,
            updated_at: self.updated_at,
            cost_per_1k_input_usd: self.cost_per_1k_input_usd,
            cost_per_1k_output_usd: self.cost_per_1k_output_usd,
        })
    }
}

const SELECT_COLUMNS: &str = "id, name, kind, endpoint, models, default_for_models, \
     fallback_order, api_key_env, api_key, created_at, updated_at, \
     cost_per_1k_input_usd, cost_per_1k_output_usd";

#[async_trait]
impl LlmProviderRepository for PgLlmProviderRepository {
    async fn create(&self, prov: &LlmProvider) -> RepoResult<()> {
        let models = serde_json::to_value(&prov.models)?;
        let defaults = serde_json::to_value(&prov.default_for_models)?;
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query(
            "INSERT INTO llm_providers \
             (id, name, kind, endpoint, models, default_for_models, \
              fallback_order, api_key_env, api_key, created_at, updated_at, \
              cost_per_1k_input_usd, cost_per_1k_output_usd) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(prov.id.as_str())
        .bind(&prov.name)
        .bind(prov.kind.as_str())
        .bind(&prov.endpoint)
        .bind(models)
        .bind(defaults)
        .bind(prov.fallback_order)
        .bind(prov.api_key_env.as_deref())
        .bind(prov.api_key.as_deref())
        .bind(prov.created_at)
        .bind(prov.updated_at)
        .bind(prov.cost_per_1k_input_usd)
        .bind(prov.cost_per_1k_output_usd)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let row = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        row.map(LlmProviderRow::into_domain).transpose()
    }

    async fn list(&self) -> RepoResult<Vec<LlmProvider>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let rows = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers \
             ORDER BY fallback_order ASC, created_at ASC"
        ))
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(LlmProviderRow::into_domain).collect()
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query("DELETE FROM llm_providers WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
