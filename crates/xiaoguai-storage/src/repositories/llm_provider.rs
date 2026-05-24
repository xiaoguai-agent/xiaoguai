//! `LlmProviderRepository` — Postgres-backed registry of LLM providers.
//!
//! A row's `tenant_id` is `None` for system-wide providers visible to every
//! tenant. RLS policy `tenant_or_global_isolation` enforces this at the DB
//! layer: rows with `tenant_id IS NULL` are always visible; tenant-scoped
//! rows require the `app.current_tenant_id` GUC to match. Every method on
//! this repo runs inside a tenant-scoped transaction via
//! [`begin_tenant_tx`]; the GUC is set when the caller supplies a tenant.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use xiaoguai_types::{
    ids::{ProviderId, TenantId},
    LlmProvider, ProviderKind,
};

use crate::repositories::error::{RepoError, RepoResult};
use crate::repositories::tenant_ctx::begin_tenant_tx;

#[async_trait]
pub trait LlmProviderRepository: Send + Sync {
    async fn create(&self, tenant: Option<&str>, prov: &LlmProvider) -> RepoResult<()>;
    async fn find_by_id(&self, tenant: Option<&str>, id: &str) -> RepoResult<Option<LlmProvider>>;
    /// Return system-wide providers (rows with `tenant_id IS NULL`), ordered
    /// by `fallback_order` ascending. Admin/cross-tenant — no GUC is set.
    async fn list_global(&self) -> RepoResult<Vec<LlmProvider>>;
    /// Return system-wide providers plus rows scoped to `tenant_id`, ordered
    /// by `fallback_order` ascending. Tenant rows come after globals when
    /// orders tie. The supplied `tenant_id` doubles as the RLS GUC value.
    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<LlmProvider>>;
    async fn delete(&self, tenant: Option<&str>, id: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct PgLlmProviderRepository {
    pool: PgPool,
}

impl PgLlmProviderRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct LlmProviderRow {
    id: String,
    tenant_id: Option<String>,
    name: String,
    kind: String,
    endpoint: String,
    models: serde_json::Value,
    default_for_models: serde_json::Value,
    fallback_order: i32,
    api_key_env: Option<String>,
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
            tenant_id: self.tenant_id.map(TenantId::from),
            name: self.name,
            kind,
            endpoint: self.endpoint,
            models,
            default_for_models,
            fallback_order: self.fallback_order,
            api_key_env: self.api_key_env,
            created_at: self.created_at,
            updated_at: self.updated_at,
            cost_per_1k_input_usd: self.cost_per_1k_input_usd,
            cost_per_1k_output_usd: self.cost_per_1k_output_usd,
        })
    }
}

const SELECT_COLUMNS: &str = "id, tenant_id, name, kind, endpoint, models, default_for_models, \
     fallback_order, api_key_env, created_at, updated_at, \
     cost_per_1k_input_usd, cost_per_1k_output_usd";

#[async_trait]
impl LlmProviderRepository for PgLlmProviderRepository {
    async fn create(&self, tenant: Option<&str>, prov: &LlmProvider) -> RepoResult<()> {
        let models = serde_json::to_value(&prov.models)?;
        let defaults = serde_json::to_value(&prov.default_for_models)?;
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        sqlx::query(
            "INSERT INTO llm_providers \
             (id, tenant_id, name, kind, endpoint, models, default_for_models, \
              fallback_order, api_key_env, created_at, updated_at, \
              cost_per_1k_input_usd, cost_per_1k_output_usd) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(prov.id.as_str())
        .bind(prov.tenant_id.as_ref().map(AsRef::as_ref))
        .bind(&prov.name)
        .bind(prov.kind.as_str())
        .bind(&prov.endpoint)
        .bind(models)
        .bind(defaults)
        .bind(prov.fallback_order)
        .bind(prov.api_key_env.as_deref())
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

    async fn find_by_id(&self, tenant: Option<&str>, id: &str) -> RepoResult<Option<LlmProvider>> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        let row = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        row.map(LlmProviderRow::into_domain).transpose()
    }

    async fn list_global(&self) -> RepoResult<Vec<LlmProvider>> {
        let mut tx = begin_tenant_tx(&self.pool, None).await?;
        let rows = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers \
             WHERE tenant_id IS NULL \
             ORDER BY fallback_order ASC, created_at ASC"
        ))
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(LlmProviderRow::into_domain).collect()
    }

    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<LlmProvider>> {
        let mut tx = begin_tenant_tx(&self.pool, Some(tenant_id)).await?;
        // Globals first (tenant_id IS NULL sorts NULLs LAST by default, so use a
        // computed key that puts globals before tenant rows when fallback_order ties).
        let rows = sqlx::query_as::<_, LlmProviderRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM llm_providers \
             WHERE tenant_id IS NULL OR tenant_id = $1 \
             ORDER BY fallback_order ASC, (tenant_id IS NOT NULL) ASC, created_at ASC"
        ))
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(LlmProviderRow::into_domain).collect()
    }

    async fn delete(&self, tenant: Option<&str>, id: &str) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
        sqlx::query("DELETE FROM llm_providers WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
