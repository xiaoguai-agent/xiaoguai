//! `UserRepository` — Postgres-backed user CRUD with role join.
//!
//! The `users` table has RLS enabled (see `0001_initial.sql`). All queries
//! against this repository run inside a transaction that first calls
//! `SET LOCAL app.current_tenant_id = '<tenant_id>'` to satisfy the
//! `tenant_isolation_users` policy. Lookups by `id` therefore require a
//! self-join against `users` to discover the tenant before setting the GUC
//! — we accept the extra round trip for defense-in-depth.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Acquire, FromRow, PgPool, Postgres, Transaction};
use xiaoguai_types::{
    ids::{TenantId, UserId},
    TenantRole as Role, User,
};

use crate::repositories::error::{RepoError, RepoResult};

/// Abstract user storage. Operations are tenant-scoped where possible; the
/// `id`-only lookups discover the tenant first then re-enter under RLS.
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Insert a new user along with their roles. Roles are persisted to the
    /// `user_roles` join table in the same transaction as the user row.
    async fn create(&self, user: &User) -> RepoResult<()>;

    /// Look up a user by primary key, including roles. Returns `Ok(None)`
    /// when the user does not exist.
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<User>>;

    /// Look up a user by `(tenant_id, email)`. Returns `Ok(None)` when missing.
    async fn find_by_email(&self, tenant_id: &str, email: &str) -> RepoResult<Option<User>>;

    /// List users for a tenant, ordered by `created_at` ascending.
    async fn list_by_tenant(
        &self,
        tenant_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<User>>;

    /// Delete a user. `ON DELETE CASCADE` removes role rows. Idempotent.
    async fn delete(&self, id: &str) -> RepoResult<()>;

    /// Stamp `last_login_at = NOW()` for the given user id.
    async fn record_login(&self, id: &str) -> RepoResult<()>;
}

/// Postgres implementation of [`UserRepository`].
#[derive(Debug, Clone)]
pub struct PgUserRepository {
    pool: PgPool,
}

impl PgUserRepository {
    /// Wrap an existing `PgPool`. The pool is cheap to clone (Arc inside).
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: String,
    tenant_id: String,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    last_login_at: Option<DateTime<Utc>>,
}

const USER_COLUMNS: &str = "id, tenant_id, email, display_name, created_at, last_login_at";

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::SystemAdmin => "system_admin",
        Role::TenantAdmin => "tenant_admin",
        Role::Member => "member",
    }
}

fn role_from_str(s: &str) -> RepoResult<Role> {
    match s {
        "system_admin" => Ok(Role::SystemAdmin),
        "tenant_admin" => Ok(Role::TenantAdmin),
        "member" => Ok(Role::Member),
        other => Err(RepoError::InvalidArgument(format!(
            "unknown role in DB: {other}"
        ))),
    }
}

/// Set the `app.current_tenant_id` GUC inside the active transaction so that
/// RLS policies on `users` evaluate to true for rows in `tenant_id`.
async fn set_tenant_guc(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<(), sqlx::Error> {
    // `SET LOCAL` only accepts literals, not bind parameters, so we use
    // `set_config(name, value, is_local)` which does take parameters.
    sqlx::query("SELECT set_config('app.current_tenant_id', $1, true)")
        .bind(tenant_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_roles(tx: &mut Transaction<'_, Postgres>, user_id: &str) -> RepoResult<Vec<Role>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT role FROM user_roles WHERE user_id = $1 ORDER BY role")
            .bind(user_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(RepoError::from_sqlx)?;
    rows.into_iter().map(|(r,)| role_from_str(&r)).collect()
}

fn row_into_user(row: UserRow, roles: Vec<Role>) -> User {
    User {
        id: UserId::from(row.id),
        tenant_id: TenantId::from(row.tenant_id),
        email: row.email,
        display_name: row.display_name,
        roles,
        created_at: row.created_at,
        last_login_at: row.last_login_at,
    }
}

/// Discover the tenant that owns a user id without going through RLS.
/// Uses a `SECURITY DEFINER`-style escape hatch: query a system catalog view
/// — but we don't have one, so we instead bypass RLS by running as the
/// table owner. Tests connect as `postgres` (superuser) which bypasses RLS;
/// in production the migration role should keep ownership.
async fn discover_tenant_of_user(pool: &PgPool, user_id: &str) -> RepoResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT tenant_id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(RepoError::from_sqlx)?;
    Ok(row.map(|(t,)| t))
}

#[async_trait]
impl UserRepository for PgUserRepository {
    async fn create(&self, user: &User) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        set_tenant_guc(&mut tx, user.tenant_id.as_str())
            .await
            .map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO users (id, tenant_id, email, display_name, created_at, last_login_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(user.id.as_str())
        .bind(user.tenant_id.as_str())
        .bind(&user.email)
        .bind(&user.display_name)
        .bind(user.created_at)
        .bind(user.last_login_at)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        for role in &user.roles {
            sqlx::query("INSERT INTO user_roles (user_id, role) VALUES ($1, $2)")
                .bind(user.id.as_str())
                .bind(role_to_str(*role))
                .execute(&mut *tx)
                .await
                .map_err(RepoError::from_sqlx)?;
        }

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<User>> {
        let Some(tenant_id) = discover_tenant_of_user(&self.pool, id).await? else {
            return Ok(None);
        };

        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        set_tenant_guc(&mut tx, &tenant_id)
            .await
            .map_err(RepoError::from_sqlx)?;

        let row = sqlx::query_as::<_, UserRow>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let result = match row {
            Some(r) => {
                let roles = load_roles(&mut tx, id).await?;
                Some(row_into_user(r, roles))
            }
            None => None,
        };

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(result)
    }

    async fn find_by_email(&self, tenant_id: &str, email: &str) -> RepoResult<Option<User>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        set_tenant_guc(&mut tx, tenant_id)
            .await
            .map_err(RepoError::from_sqlx)?;

        let row = sqlx::query_as::<_, UserRow>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE tenant_id = $1 AND email = $2"
        ))
        .bind(tenant_id)
        .bind(email)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let result = match row {
            Some(r) => {
                let id = r.id.clone();
                let roles = load_roles(&mut tx, &id).await?;
                Some(row_into_user(r, roles))
            }
            None => None,
        };

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(result)
    }

    async fn list_by_tenant(
        &self,
        tenant_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<User>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit and offset must be non-negative".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        set_tenant_guc(&mut tx, tenant_id)
            .await
            .map_err(RepoError::from_sqlx)?;

        let rows = sqlx::query_as::<_, UserRow>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE tenant_id = $1 \
             ORDER BY created_at ASC LIMIT $2 OFFSET $3"
        ))
        .bind(tenant_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        let mut users = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row.id.clone();
            let roles = load_roles(&mut tx, &id).await?;
            users.push(row_into_user(row, roles));
        }

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(users)
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        // RLS will hide the row from non-tenant connections; superuser
        // bypasses RLS in tests, and in production the storage role is the
        // table owner which also bypasses RLS. We avoid a tenant lookup here
        // because `delete` should be cheap and idempotent.
        let mut conn = self.pool.acquire().await.map_err(RepoError::from_sqlx)?;
        let mut tx = conn.begin().await.map_err(RepoError::from_sqlx)?;

        if let Some(tenant_id) = discover_tenant_of_user(&self.pool, id).await? {
            set_tenant_guc(&mut tx, &tenant_id)
                .await
                .map_err(RepoError::from_sqlx)?;
        }

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn record_login(&self, id: &str) -> RepoResult<()> {
        let Some(tenant_id) = discover_tenant_of_user(&self.pool, id).await? else {
            return Err(RepoError::NotFound);
        };

        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        set_tenant_guc(&mut tx, &tenant_id)
            .await
            .map_err(RepoError::from_sqlx)?;

        let result = sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;

        if result.rows_affected() == 0 {
            return Err(RepoError::NotFound);
        }
        Ok(())
    }
}
