//! sprint-13 S13-3: `HotlRedactionRepo` — read-only per-tenant access to
//! `hotl_redaction_policies` (DEC-HLD-014, guardrails.md §3.1).
//!
//! Admin CRUD (create / update / delete) lands in sprint-14 alongside the
//! admin-ui surface. This repo is **deliberately read-only** so the S13-4
//! caller (`xiaoguai-auth::redaction::RedactionRules::from_storage`) can't be
//! tempted to mutate rules at request time — that path must go through the
//! admin API once it exists.
//!
//! ## RLS
//!
//! `hotl_redaction_policies` is RLS-enabled (migration 0027). The policy
//! references `app.current_tenant_id`; this repo sets that GUC inside the
//! same transaction as the SELECT via [`begin_tenant_tx`]. The caller passes
//! the tenant UUID directly — no string conversion at the call site.
//!
//! ## Ordering
//!
//! Results are sorted exact-scope first, then `*` catch-all. The S13-4
//! consumer iterates and picks the first matching rule for a given scope,
//! so the `*` rule only applies when no exact match exists.

use chrono::{DateTime, Utc};
use sqlx::{types::Json, FromRow, SqlitePool};
use uuid::Uuid;

use crate::repositories::error::{RepoError, RepoResult};
use crate::repositories::tenant_ctx::begin_tenant_tx;

/// A single row from `hotl_redaction_policies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionPolicyRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// `*` is the catch-all; any other value is an exact scope match
    /// (e.g. `tool_call.execute_python`).
    pub scope: String,
    /// `JSONPath` selector (e.g. `$.password`, `$.headers.authorization`).
    pub jsonpath: String,
    /// Where the redaction applies — typical values are `sse` and `audit`.
    pub applies_to: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// `FromRow` shim. The `tenant_id` column is gone under the single-user pivot,
/// and `applies_to` is stored as a JSON-array TEXT column, so it can't decode
/// straight into `Vec<String>` — the `Json<T>` wrapper handles it. The public
/// [`RedactionPolicyRow`] is reconstructed in [`PgHotlRedactionRepo::load_for_tenant`].
#[derive(Debug, FromRow)]
struct RedactionPolicyDbRow {
    id: Uuid,
    scope: String,
    jsonpath: String,
    applies_to: Json<Vec<String>>,
    created_at: DateTime<Utc>,
}

/// Read-only access to `hotl_redaction_policies`.
///
/// Implemented as a trait so S13-4 unit tests can swap in a hand-rolled
/// in-memory fake without spinning up Postgres.
#[async_trait::async_trait]
pub trait HotlRedactionRepo: Send + Sync {
    /// Return every policy row for `tenant_id`, sorted exact-scope first
    /// then `*` catch-all, with a secondary sort on `scope ASC` for
    /// determinism when multiple exact scopes coexist.
    async fn load_for_tenant(&self, tenant_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>>;
}

/// SQLite-backed implementation.
#[derive(Debug, Clone)]
pub struct PgHotlRedactionRepo {
    pool: SqlitePool,
}

impl PgHotlRedactionRepo {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl HotlRedactionRepo for PgHotlRedactionRepo {
    async fn load_for_tenant(&self, tenant_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>> {
        // Single namespace under the pivot: every policy row is owner-wide.
        // The vestigial `tenant_id` arg is echoed back onto each row so the
        // public shape is preserved for downstream consumers.
        let tenant_str = tenant_id.to_string();
        let mut tx = begin_tenant_tx(&self.pool, Some(&tenant_str)).await?;
        let rows = sqlx::query_as::<_, RedactionPolicyDbRow>(
            "SELECT id, scope, jsonpath, applies_to, created_at \
             FROM hotl_redaction_policies \
             ORDER BY (scope = '*') ASC, scope ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|r| RedactionPolicyRow {
                id: r.id,
                tenant_id,
                scope: r.scope,
                jsonpath: r.jsonpath,
                applies_to: r.applies_to.0,
                created_at: r.created_at,
            })
            .collect())
    }
}
