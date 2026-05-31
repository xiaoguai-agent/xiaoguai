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
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::repositories::error::{RepoError, RepoResult};
use crate::repositories::tenant_ctx::begin_tenant_tx;

/// A single row from `hotl_redaction_policies`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
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

/// Postgres-backed implementation.
#[derive(Debug, Clone)]
pub struct PgHotlRedactionRepo {
    pool: PgPool,
}

impl PgHotlRedactionRepo {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl HotlRedactionRepo for PgHotlRedactionRepo {
    async fn load_for_tenant(&self, tenant_id: Uuid) -> RepoResult<Vec<RedactionPolicyRow>> {
        // `app.current_tenant_id` is a TEXT GUC; the RLS policy compares
        // `tenant_id::text = current_setting(...)`. Format the UUID as a
        // plain hyphenated string to match Postgres's default UUID cast.
        let tenant_str = tenant_id.to_string();
        let mut tx = begin_tenant_tx(&self.pool, Some(&tenant_str)).await?;
        let rows = sqlx::query_as::<_, RedactionPolicyRow>(
            "SELECT id, tenant_id, scope, jsonpath, applies_to, created_at \
             FROM hotl_redaction_policies \
             WHERE tenant_id = $1 \
             ORDER BY (scope = '*') ASC, scope ASC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(rows)
    }
}
