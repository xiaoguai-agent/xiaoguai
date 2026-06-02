//! Transaction helper — vestigial tenant scoping (DEC-033 single-user).
//!
//! Under Postgres every RLS-protected query ran inside a transaction that set
//! the `app.current_tenant_id` GUC. SQLite has no RLS and the single-user pivot
//! has one implicit owner, so there is nothing to scope: [`begin_tenant_tx`]
//! now just opens a plain transaction and ignores the `tenant` argument.
//!
//! The signature is retained so the repository bodies (which call
//! `begin_tenant_tx(&self.pool, tenant)`) keep compiling unchanged; the
//! `tenant` parameter on the repository trait methods is likewise vestigial.

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::repositories::error::{RepoError, RepoResult};

/// Open a transaction. The `tenant` argument is ignored (no RLS under SQLite).
///
/// The returned transaction must be committed by the caller; dropping it
/// without committing rolls back.
pub async fn begin_tenant_tx<'p>(
    pool: &'p SqlitePool,
    _tenant: Option<&str>,
) -> RepoResult<Transaction<'p, Sqlite>> {
    pool.begin().await.map_err(RepoError::from_sqlx)
}
