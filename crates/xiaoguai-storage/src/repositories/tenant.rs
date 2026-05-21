//! `TenantRepository` — Postgres-backed tenant CRUD.
//!
//! Tenants are a cross-tenant resource (the "directory" of tenants) and are
//! therefore not subject to the row-level security policies that gate the
//! per-tenant tables (`users`, `sessions`, `messages`). The `tenants` table has
//! no RLS policy in `0001_initial.sql`; we connect as a Postgres superuser in
//! production and tests.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use xiaoguai_types::{ids::TenantId, Tenant, TenantStatus};

use crate::repositories::error::{RepoError, RepoResult};

/// Abstract tenant storage. See module docs for RLS notes.
#[async_trait]
pub trait TenantRepository: Send + Sync {
    /// Insert a new tenant. Returns [`RepoError::DuplicateKey`] when a tenant
    /// with the same `id` or `name` already exists.
    async fn create(&self, tenant: &Tenant) -> RepoResult<()>;

    /// Look up a tenant by its primary key. Returns `Ok(None)` when missing.
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Tenant>>;

    /// Look up a tenant by its unique `name`. Returns `Ok(None)` when missing.
    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Tenant>>;

    /// List tenants ordered by `created_at` ascending.
    async fn list(&self, limit: i64, offset: i64) -> RepoResult<Vec<Tenant>>;

    /// Delete a tenant by id. Idempotent: returns `Ok(())` when the tenant is
    /// already gone. `ON DELETE CASCADE` propagates to users/sessions/messages.
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

/// Postgres implementation of [`TenantRepository`].
#[derive(Debug, Clone)]
pub struct PgTenantRepository {
    pool: PgPool,
}

impl PgTenantRepository {
    /// Wrap an existing `PgPool`. The pool is cheap to clone (Arc inside).
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Raw row pulled from the `tenants` table.
#[derive(Debug, FromRow)]
struct TenantRow {
    id: String,
    name: String,
    display_name: String,
    status: String,
    created_at: DateTime<Utc>,
}

impl TenantRow {
    fn into_domain(self) -> RepoResult<Tenant> {
        let status = match self.status.as_str() {
            "active" => TenantStatus::Active,
            "suspended" => TenantStatus::Suspended,
            "archived" => TenantStatus::Archived,
            other => {
                return Err(RepoError::InvalidArgument(format!(
                    "unknown tenant status in DB: {other}"
                )));
            }
        };
        Ok(Tenant {
            id: TenantId::from(self.id),
            name: self.name,
            display_name: self.display_name,
            created_at: self.created_at,
            status,
        })
    }
}

fn status_as_str(status: TenantStatus) -> &'static str {
    match status {
        TenantStatus::Active => "active",
        TenantStatus::Suspended => "suspended",
        TenantStatus::Archived => "archived",
    }
}

const SELECT_COLUMNS: &str = "id, name, display_name, status, created_at";

#[async_trait]
impl TenantRepository for PgTenantRepository {
    async fn create(&self, tenant: &Tenant) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO tenants (id, name, display_name, status, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(tenant.id.as_str())
        .bind(&tenant.name)
        .bind(&tenant.display_name)
        .bind(status_as_str(tenant.status))
        .bind(tenant.created_at)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<Tenant>> {
        let row = sqlx::query_as::<_, TenantRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM tenants WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        row.map(TenantRow::into_domain).transpose()
    }

    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Tenant>> {
        let row = sqlx::query_as::<_, TenantRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM tenants WHERE name = $1"
        ))
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        row.map(TenantRow::into_domain).transpose()
    }

    async fn list(&self, limit: i64, offset: i64) -> RepoResult<Vec<Tenant>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit and offset must be non-negative".to_string(),
            ));
        }
        let rows = sqlx::query_as::<_, TenantRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM tenants ORDER BY created_at ASC LIMIT $1 OFFSET $2"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(TenantRow::into_domain).collect()
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
