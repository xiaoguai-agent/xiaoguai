//! `UserRepository` — SQLite-backed user CRUD with role join.
//!
//! Single-user pivot (DEC-033): the `users` table dropped its `tenant_id`
//! column and row-level security; `email` is globally unique. The vestigial
//! `tenant_id` arguments on `find_by_email` / `list_by_tenant` are ignored,
//! and the required `User::tenant_id` domain field is synthesised from
//! [`OWNER_TENANT_ID`] on read.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};
use xiaoguai_types::{
    ids::{TenantId, UserId},
    TenantRole as Role, User,
};

use crate::repositories::error::{RepoError, RepoResult};
use crate::OWNER_TENANT_ID;

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

/// `SQLite` implementation of [`UserRepository`].
#[derive(Debug, Clone)]
pub struct PgUserRepository {
    pool: SqlitePool,
}

impl PgUserRepository {
    /// Wrap an existing `SqlitePool`. The pool is cheap to clone (Arc inside).
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: String,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    last_login_at: Option<DateTime<Utc>>,
}

const USER_COLUMNS: &str = "id, email, display_name, created_at, last_login_at";

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

async fn load_roles(tx: &mut Transaction<'_, Sqlite>, user_id: &str) -> RepoResult<Vec<Role>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT role FROM user_roles WHERE user_id = ? ORDER BY role")
            .bind(user_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(RepoError::from_sqlx)?;
    rows.into_iter().map(|(r,)| role_from_str(&r)).collect()
}

fn row_into_user(row: UserRow, roles: Vec<Role>) -> User {
    User {
        id: UserId::from(row.id),
        tenant_id: TenantId::from(OWNER_TENANT_ID.to_string()),
        email: row.email,
        display_name: row.display_name,
        roles,
        created_at: row.created_at,
        last_login_at: row.last_login_at,
    }
}

#[async_trait]
impl UserRepository for PgUserRepository {
    async fn create(&self, user: &User) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO users (id, email, display_name, created_at, last_login_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(user.id.as_str())
        .bind(&user.email)
        .bind(&user.display_name)
        .bind(user.created_at)
        .bind(user.last_login_at)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        for role in &user.roles {
            sqlx::query("INSERT INTO user_roles (user_id, role) VALUES (?, ?)")
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
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let row =
            sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
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

    async fn find_by_email(&self, _tenant_id: &str, email: &str) -> RepoResult<Option<User>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let row = sqlx::query_as::<_, UserRow>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE email = ?"
        ))
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
        _tenant_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<User>> {
        if limit < 0 || offset < 0 {
            return Err(RepoError::InvalidArgument(
                "limit and offset must be non-negative".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let rows = sqlx::query_as::<_, UserRow>(&format!(
            "SELECT {USER_COLUMNS} FROM users \
             ORDER BY created_at ASC LIMIT ? OFFSET ?"
        ))
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
        // Idempotent — deleting a non-existent row is not an error.
        // `ON DELETE CASCADE` removes the user's role rows.
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn record_login(&self, id: &str) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        let result = sqlx::query(
            "UPDATE users SET last_login_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
        )
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
