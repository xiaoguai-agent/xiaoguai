//! sprint-13 S13-3: `HotlRedactionRepo` — read-only access to
//! `hotl_redaction_policies` (DEC-HLD-014, guardrails.md §3.1).
//!
//! Admin CRUD (create / update / delete) lands in sprint-14 alongside the
//! admin-ui surface. This repo is **deliberately read-only** so the S13-4
//! caller (`xiaoguai-auth::redaction::RedactionRules::from_storage`) can't be
//! tempted to mutate rules at request time — that path must go through the
//! admin API once it exists.
//!
//! Single-owner deployment (DEC-033): no tenant scoping — every read returns
//! all policy rows.
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

/// A single row from `hotl_redaction_policies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionPolicyRow {
    pub id: Uuid,
    /// `*` is the catch-all; any other value is an exact scope match
    /// (e.g. `tool_call.execute_python`).
    pub scope: String,
    /// `JSONPath` selector (e.g. `$.password`, `$.headers.authorization`).
    pub jsonpath: String,
    /// Where the redaction applies — typical values are `sse` and `audit`.
    pub applies_to: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// `FromRow` shim. `applies_to` is stored as a JSON-array TEXT column, so it
/// can't decode straight into `Vec<String>` — the `Json<T>` wrapper handles
/// it. The public [`RedactionPolicyRow`] is reconstructed in
/// [`PgHotlRedactionRepo::load_all`].
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
/// in-memory fake without spinning up a database.
#[async_trait::async_trait]
pub trait HotlRedactionRepo: Send + Sync {
    /// Return every policy row, sorted exact-scope first then `*` catch-all,
    /// with a secondary sort on `scope ASC` for determinism when multiple
    /// exact scopes coexist.
    async fn load_all(&self) -> RepoResult<Vec<RedactionPolicyRow>>;
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
    async fn load_all(&self) -> RepoResult<Vec<RedactionPolicyRow>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
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
                scope: r.scope,
                jsonpath: r.jsonpath,
                applies_to: r.applies_to.0,
                created_at: r.created_at,
            })
            .collect())
    }
}
