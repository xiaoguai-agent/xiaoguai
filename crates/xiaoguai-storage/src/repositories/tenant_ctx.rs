//! Per-request tenant context helper for RLS-aware queries.
//!
//! Every Postgres operation against an RLS-enabled table (`users`,
//! `sessions`, `messages`, `llm_providers`, `token_usage`, `mcp_servers`)
//! must run inside a transaction that has set
//! `app.current_tenant_id` to the caller's tenant. The policies reference
//! that GUC; without it set, a non-superuser role sees an empty result.
//!
//! Pattern used by every repo method touching one of those tables:
//!
//! ```ignore
//! let mut tx = begin_tenant_tx(&self.pool, tenant).await?;
//! sqlx::query("...").bind(...).execute(&mut *tx).await?;
//! tx.commit().await?;
//! ```
//!
//! When `tenant` is `None` the transaction is still opened but no GUC is
//! set. That path exists for two callers:
//!
//! 1. Admin / cross-tenant CLI operations (e.g. `xiaoguai provider list`)
//!    that should bypass tenant filtering. In production these run as a
//!    superuser-ish role; in tests the container's `postgres` user
//!    bypasses non-FORCE policies.
//! 2. The `tenants` table itself, which has no RLS — it's the registry
//!    used to bootstrap tenants. Repos for that table take no tenant arg.
//!
//! `SELECT set_config(name, value, is_local=true)` is preferred over the
//! statement form `SET LOCAL` because it accepts a bound parameter — `SET`
//! is a parse-time keyword and cannot be parameterised.

use sqlx::{PgPool, Postgres, Transaction};

use crate::repositories::error::{RepoError, RepoResult};

/// Open a transaction and, when `tenant` is `Some`, scope it to that
/// tenant via `set_config('app.current_tenant_id', $1, true)`.
///
/// The returned transaction must be committed by the caller; dropping it
/// without committing rolls back, which is fine for read-only paths but
/// will lose writes.
pub async fn begin_tenant_tx<'p>(
    pool: &'p PgPool,
    tenant: Option<&str>,
) -> RepoResult<Transaction<'p, Postgres>> {
    let mut tx = pool.begin().await.map_err(RepoError::from_sqlx)?;
    if let Some(t) = tenant {
        sqlx::query("SELECT set_config('app.current_tenant_id', $1, true)")
            .bind(t)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
    }
    Ok(tx)
}
