//! `HotlEscalationStore` — Postgres-backed repo for the
//! `hotl_escalations` (parent) + `hotl_pending` (child) tables shipped by
//! migration 0027 (sprint-13 S13-1).
//!
//! The trait lives in `xiaoguai-storage` rather than `xiaoguai-auth` per
//! DEC-LLD-AGENT-005 so that `xiaoguai-core::run_serve` can depend on it
//! for boot replay without pulling in `xiaoguai-auth`'s policy graph.
//!
//! Three operations make up the full surface:
//!
//! 1. [`insert_pending`](HotlEscalationStore::insert_pending) — atomic
//!    2-row write. The parent escalation row is `INSERT`-ed first; its `id`
//!    is then bound as the child's `escalation_id` FK. Wrapped in a
//!    single `sqlx::Transaction` so a crash mid-write leaves no orphan
//!    rows (validates the FK NOT NULL invariant locked in by migration
//!    0027).
//!
//! 2. [`list_pending_unexpired`](HotlEscalationStore::list_pending_unexpired)
//!    — the boot-replay query. Runs **without** tenant context because
//!    the registry is rebuilt before any per-tenant request is served;
//!    the returned rows carry `tenant_id` so callers can scope downstream
//!    work. The supporting partial index
//!    `hotl_pending_status_expires_idx` (migration 0027) makes the scan
//!    cheap even at high pending-row counts.
//!
//! 3. [`record_decision`](HotlEscalationStore::record_decision) — the
//!    `UPDATE` fired when a `HotL` verdict arrives. Returns whether a row
//!    actually matched (via `rows_affected() > 0`) so the caller can
//!    distinguish "decision applied" from "row already resolved or
//!    expired by another worker / boot replay" and degrade gracefully.
//!
//! No `tracing` logs live here — the registry layer logs at decision
//! time. Keeping the repo silent makes it trivial to embed in tests
//! that already assert on logs upstream.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

use crate::repositories::error::{RepoError, RepoResult};

/// Verdict applied by the operator (or by the boot replay's "expired"
/// synthesis path) when resolving a pending `HotL` escalation.
///
/// Maps to the `status` CHECK constraint on `hotl_pending` from migration
/// 0027 — only `resolved` and `expired` are terminal states; `pending` is
/// reserved for the initial INSERT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotlDecisionVerdict {
    /// Operator approved the call — the agent should proceed.
    Allowed,
    /// Operator denied the call — the agent should abort the tool invocation.
    Denied,
    /// The escalation aged out without a decision (boot-replay path).
    Expired,
}

impl HotlDecisionVerdict {
    /// Postgres-side `status` string. Both `Allowed` and `Denied` map to
    /// `resolved` (the row reached a terminal decided state); `Expired`
    /// keeps a distinct value so the boot-replay synthesis path is
    /// auditable in the DB.
    #[must_use]
    pub fn status_str(self) -> &'static str {
        match self {
            Self::Allowed | Self::Denied => "resolved",
            Self::Expired => "expired",
        }
    }
}

/// Domain-shaped row for `hotl_escalations` (parent table).
///
/// Mirrors the migration 0027 schema 1-to-1. `parent_id` is `Some` only
/// for nested escalations spawned inside a triangle gate; top-level rows
/// have `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotlEscalationRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub session_id: Uuid,
    pub top_level_scope: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub parent_id: Option<Uuid>,
}

/// Domain-shaped row for `hotl_pending` (child table).
///
/// `args_redacted` is the JSONB blob produced by `RedactionRules` (S13-4)
/// — the redaction happens **upstream** of this repo, which writes the
/// blob verbatim. `decided_at`/`decided_by` are `None` until a verdict
/// lands via [`HotlEscalationStore::record_decision`].
#[derive(Debug, Clone)]
pub struct HotlPendingRow {
    pub id: Uuid,
    pub escalation_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: String,
    pub tool: String,
    pub args_redacted: JsonValue,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
}

/// `FromRow` shim — sqlx can't derive directly onto `HotlPendingRow`
/// because `JsonValue` needs the `Json<T>` wrapper to participate in
/// `FromRow`. Kept private; the `Into` conversion below is what callers
/// see.
#[derive(Debug, FromRow)]
struct HotlPendingDbRow {
    id: Uuid,
    escalation_id: Uuid,
    tenant_id: Uuid,
    scope: String,
    tool: String,
    args_redacted: Json<JsonValue>,
    status: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
    decided_by: Option<String>,
}

impl From<HotlPendingDbRow> for HotlPendingRow {
    fn from(row: HotlPendingDbRow) -> Self {
        Self {
            id: row.id,
            escalation_id: row.escalation_id,
            tenant_id: row.tenant_id,
            scope: row.scope,
            tool: row.tool,
            args_redacted: row.args_redacted.0,
            status: row.status,
            expires_at: row.expires_at,
            created_at: row.created_at,
            decided_at: row.decided_at,
            decided_by: row.decided_by,
        }
    }
}

/// Trait surface used by `DecisionRegistry` (S13-5) and the boot-replay
/// path. Object-safe (`Send + Sync` bounds + no generics on methods) so
/// `AppState` can hold an `Arc<dyn HotlEscalationStore>` without
/// committing to the concrete `PgHotlEscalationRepository`.
#[async_trait]
pub trait HotlEscalationStore: Send + Sync {
    /// Atomic 2-row write: parent first, then child with the parent's
    /// `id` bound as `escalation_id`. Returns the parent id (which is
    /// the canonical `escalation_id` used by the SSE wire contract).
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
    ) -> RepoResult<Uuid>;

    /// Boot-replay scan. Returns every `hotl_pending` row that is still
    /// `status='pending'` and has `expires_at > now`. **Not** tenant
    /// scoped — runs as the superuser at boot before any tenant GUC is
    /// set; downstream callers use the per-row `tenant_id` to route the
    /// reattached waiters.
    async fn list_pending_unexpired(&self, now: DateTime<Utc>) -> RepoResult<Vec<HotlPendingRow>>;

    /// UPDATE-the-decision path: stamps `status`/`decided_at`/`decided_by`
    /// onto the matching `hotl_pending` row IF AND ONLY IF it is still in
    /// `pending` state. Returns `Ok(true)` when a row was updated and
    /// `Ok(false)` when nothing matched (unknown id, already-resolved row,
    /// or a race lost to the boot-replay timeout sweep).
    async fn record_decision(
        &self,
        escalation_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: Option<String>,
    ) -> RepoResult<bool>;
}

/// Postgres implementation backed by sqlx.
///
/// Holds the `PgPool` directly rather than going through
/// [`begin_tenant_tx`] because every operation here is either (a) a
/// 2-row write the caller has already tenant-scoped at a higher layer
/// (the parent row carries `tenant_id`), or (b) the boot-replay scan
/// which is by design cross-tenant.
#[derive(Debug, Clone)]
pub struct PgHotlEscalationRepository {
    pool: PgPool,
}

impl PgHotlEscalationRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const PENDING_COLUMNS: &str = "id, escalation_id, tenant_id, scope, tool, args_redacted, \
                               status, expires_at, created_at, decided_at, decided_by";

#[async_trait]
impl HotlEscalationStore for PgHotlEscalationRepository {
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
    ) -> RepoResult<Uuid> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO hotl_escalations \
             (id, tenant_id, session_id, top_level_scope, status, created_at, parent_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(parent.id)
        .bind(parent.tenant_id)
        .bind(parent.session_id)
        .bind(&parent.top_level_scope)
        .bind(&parent.status)
        .bind(parent.created_at)
        .bind(parent.parent_id)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO hotl_pending \
             (id, escalation_id, tenant_id, scope, tool, args_redacted, status, \
              expires_at, created_at, decided_at, decided_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(child.id)
        // Force the FK to match the parent we just wrote — ignore whatever
        // the caller put in `child.escalation_id` so the round-trip
        // invariant "child.escalation_id == returned parent id" is
        // unconditional.
        .bind(parent.id)
        .bind(child.tenant_id)
        .bind(&child.scope)
        .bind(&child.tool)
        .bind(Json(&child.args_redacted))
        .bind(&child.status)
        .bind(child.expires_at)
        .bind(child.created_at)
        .bind(child.decided_at)
        .bind(child.decided_by.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;

        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(parent.id)
    }

    async fn list_pending_unexpired(&self, now: DateTime<Utc>) -> RepoResult<Vec<HotlPendingRow>> {
        let rows: Vec<HotlPendingDbRow> = sqlx::query_as(&format!(
            "SELECT {PENDING_COLUMNS} FROM hotl_pending \
             WHERE status = 'pending' AND expires_at > $1 \
             ORDER BY created_at ASC"
        ))
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(rows.into_iter().map(HotlPendingRow::from).collect())
    }

    async fn record_decision(
        &self,
        escalation_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: Option<String>,
    ) -> RepoResult<bool> {
        let result = sqlx::query(
            "UPDATE hotl_pending \
             SET status = $1, decided_by = $2, decided_at = now() \
             WHERE escalation_id = $3 AND status = 'pending'",
        )
        .bind(verdict.status_str())
        .bind(decided_by.as_deref())
        .bind(escalation_id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }
}
