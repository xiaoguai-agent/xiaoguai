//! `TenantRepository` — synthetic single-owner stub (DEC-033 single-user).
//!
//! The `tenants` table is gone under the single-user pivot. The trait is kept so
//! `AppState` (which exposes an optional `/v1/admin/tenants` route) and the
//! bootstrap in `xiaoguai-core` keep compiling, but there is exactly one
//! implicit owner. Lookups return that synthetic owner; `create`/`delete` are
//! no-ops. A later cleanup may drop the trait and route entirely.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::SqlitePool;
use xiaoguai_types::{ids::TenantId, Tenant, TenantStatus};

use crate::repositories::error::RepoResult;
use crate::OWNER_TENANT_ID;

/// Abstract tenant storage. Single-owner under DEC-033.
#[async_trait]
pub trait TenantRepository: Send + Sync {
    /// No-op: there is one implicit owner.
    async fn create(&self, tenant: &Tenant) -> RepoResult<()>;

    /// Returns the synthetic owner when `id` is the owner id, else `None`.
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Tenant>>;

    /// Returns the synthetic owner (single-user: any name resolves to it).
    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Tenant>>;

    /// Lists the single owner tenant.
    async fn list(&self, limit: i64, offset: i64) -> RepoResult<Vec<Tenant>>;

    /// No-op: the owner cannot be deleted.
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

/// SQLite-era stub implementation of [`TenantRepository`].
#[derive(Debug, Clone)]
pub struct PgTenantRepository {
    #[allow(dead_code)]
    pool: SqlitePool,
}

impl PgTenantRepository {
    /// Wrap a pool (unused; kept for call-site parity).
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Build the one synthetic owner tenant.
fn owner_tenant() -> Tenant {
    Tenant {
        id: TenantId::from(OWNER_TENANT_ID.to_string()),
        name: "local".to_string(),
        display_name: "Local Owner".to_string(),
        created_at: Utc::now(),
        status: TenantStatus::Active,
    }
}

#[async_trait]
impl TenantRepository for PgTenantRepository {
    async fn create(&self, _tenant: &Tenant) -> RepoResult<()> {
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Tenant>> {
        Ok((id == OWNER_TENANT_ID).then(owner_tenant))
    }

    async fn find_by_name(&self, _name: &str) -> RepoResult<Option<Tenant>> {
        Ok(Some(owner_tenant()))
    }

    async fn list(&self, limit: i64, offset: i64) -> RepoResult<Vec<Tenant>> {
        Ok(if offset > 0 || limit == 0 {
            vec![]
        } else {
            vec![owner_tenant()]
        })
    }

    async fn delete(&self, _id: &str) -> RepoResult<()> {
        Ok(())
    }
}
