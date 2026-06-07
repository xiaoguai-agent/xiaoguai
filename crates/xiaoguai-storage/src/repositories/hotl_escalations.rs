//! `HotlEscalationStore` — `SQLite`-backed repo for the
//! `hotl_escalations` (parent) + `hotl_pending` (child) tables shipped by
//! migration 0027 (sprint-13 S13-1).
//!
//! The trait lives in `xiaoguai-storage` rather than `xiaoguai-auth` per
//! DEC-LLD-AGENT-005 so that `xiaoguai-core::run_serve` can depend on it
//! for boot replay without pulling in `xiaoguai-auth`'s policy graph.
//!
//! Five operations make up the full surface:
//!
//! 1. [`insert_pending`](HotlEscalationStore::insert_pending) — atomic
//!    2-row write. The parent escalation row is `INSERT`-ed first; its `id`
//!    is then bound as the child's `escalation_id` FK. Wrapped in a
//!    single `sqlx::Transaction` so a crash mid-write leaves no orphan
//!    rows (validates the FK NOT NULL invariant locked in by migration
//!    0027).
//!
//! 2. [`list_pending_unexpired`](HotlEscalationStore::list_pending_unexpired)
//!    — the boot-replay query. The supporting partial index
//!    `hotl_pending_status_expires_idx` (migration 0027) makes the scan
//!    cheap even at high pending-row counts.
//!
//! 3. [`record_decision`](HotlEscalationStore::record_decision) — the
//!    `UPDATE` fired when a `HotL` verdict arrives. Returns whether a row
//!    actually matched (via `rows_affected() > 0`) so the caller can
//!    distinguish "decision applied" from "row already resolved or
//!    expired by another worker / boot replay" and degrade gracefully.
//!
//! 4. [`lookup`](HotlEscalationStore::lookup) — point lookup powering
//!    the decision route's pre-flight existence check (audit F1b);
//!    default-`Unsupported` so legacy/test stores opt out.
//!
//! 5. [`terminalise`](HotlEscalationStore::terminalise) — the timeout
//!    sweep's `UPDATE` (no `expires_at` guard); default no-op.
//!
//! No `tracing` logs live here — the registry layer logs at decision
//! time. Keeping the repo silent makes it trivial to embed in tests
//! that already assert on logs upstream.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::{types::Json, FromRow, SqlitePool};
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
    /// stored `status` string. Both `Allowed` and `Denied` map to
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

/// Result of a point lookup of a single escalation row — powers the
/// `POST /v1/hotl/decisions` pre-flight existence check (audit F1b):
/// unknown escalation ids return 404 instead of silently recording a
/// phantom decision row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscalationLookup {
    /// The store has no lookup capability (trait default — Noop/test
    /// stores). Callers MUST skip the pre-flight check and keep the
    /// legacy behaviour.
    Unsupported,
    /// No `hotl_pending` row exists for this escalation id.
    NotFound,
    /// The row exists, is `status='pending'` and unexpired — a decision
    /// can still be recorded against it.
    Pending,
    /// The row exists but can no longer accept a decision. `status` is
    /// the stored terminal status (`resolved` / `expired`); a still-
    /// `pending` row whose `expires_at` already passed is reported as
    /// `expired` (mirrors [`HotlEscalationStore::record_decision`]'s
    /// `expires_at > now` guard, which would reject it anyway). `at` is
    /// `decided_at` when stamped, else `expires_at`.
    Terminal { status: String, at: DateTime<Utc> },
}

/// Domain-shaped row for `hotl_escalations` (parent table).
///
/// Mirrors the migration 0027 schema 1-to-1. `parent_id` is `Some` only
/// for nested escalations spawned inside a triangle gate; top-level rows
/// have `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotlEscalationRow {
    pub id: Uuid,
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
/// committing to the concrete `SqliteHotlEscalationRepository`.
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
    /// `status='pending'` and has `expires_at > now`.
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

    /// Point lookup powering the decision route's pre-flight existence
    /// check (audit F1b). The default returns
    /// [`EscalationLookup::Unsupported`] so legacy/test stores
    /// (`NoopHotlEscalationStore`, pre-existing mocks) keep the
    /// always-201 route behaviour without code changes.
    async fn lookup(&self, _escalation_id: Uuid) -> RepoResult<EscalationLookup> {
        Ok(EscalationLookup::Unsupported)
    }

    /// Terminalisation path used by the registry's timeout companion
    /// tasks (audit F1b/c): stamps a terminal `status` onto a row that is
    /// still `pending`, REGARDLESS of `expires_at` — unlike
    /// [`HotlEscalationStore::record_decision`], whose `expires_at > now`
    /// guard would make it a no-op on exactly the timed-out rows this
    /// method exists to sweep. Returns `Ok(true)` when a row was updated.
    /// The default is a no-op so legacy/test stores keep compiling.
    async fn terminalise(
        &self,
        _escalation_id: Uuid,
        _verdict: HotlDecisionVerdict,
    ) -> RepoResult<bool> {
        Ok(false)
    }
}

/// `SQLite` implementation backed by sqlx.
#[derive(Debug, Clone)]
pub struct SqliteHotlEscalationRepository {
    pool: SqlitePool,
}

impl SqliteHotlEscalationRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const PENDING_COLUMNS: &str = "id, escalation_id, scope, tool, args_redacted, \
                               status, expires_at, created_at, decided_at, decided_by";

#[async_trait]
impl HotlEscalationStore for SqliteHotlEscalationRepository {
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
    ) -> RepoResult<Uuid> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;

        sqlx::query(
            "INSERT INTO hotl_escalations \
             (id, session_id, top_level_scope, status, created_at, parent_id) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(parent.id)
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
             (id, escalation_id, scope, tool, args_redacted, status, \
              expires_at, created_at, decided_at, decided_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(child.id)
        // Force the FK to match the parent we just wrote — ignore whatever
        // the caller put in `child.escalation_id` so the round-trip
        // invariant "child.escalation_id == returned parent id" is
        // unconditional.
        .bind(parent.id)
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
             WHERE status = 'pending' AND expires_at > ? \
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
        // The `expires_at > ?` guard (bound as a `DateTime`, mirroring
        // `list_pending_unexpired` so the encoding matches the stored value)
        // means a TIMED-OUT escalation can no longer be stamped 'resolved': once
        // its deadline passed, a late operator decision matches no row and
        // returns Ok(false). Without it, because the timeout path leaves the row
        // `status='pending'`, a late decision would falsely flip an
        // already-abandoned escalation to resolved in the audit/DB.
        let result = sqlx::query(
            "UPDATE hotl_pending \
             SET status = ?, decided_by = ?, decided_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE escalation_id = ? AND status = 'pending' AND expires_at > ?",
        )
        .bind(verdict.status_str())
        .bind(decided_by.as_deref())
        .bind(escalation_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }

    async fn lookup(&self, escalation_id: Uuid) -> RepoResult<EscalationLookup> {
        let row: Option<(String, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT status, expires_at, decided_at FROM hotl_pending WHERE escalation_id = ?",
        )
        .bind(escalation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        let Some((status, expires_at, decided_at)) = row else {
            return Ok(EscalationLookup::NotFound);
        };
        if status == "pending" {
            if expires_at > Utc::now() {
                return Ok(EscalationLookup::Pending);
            }
            // Pending-but-expired: the timeout sweep hasn't stamped it yet,
            // but `record_decision`'s `expires_at > now` guard would reject
            // a decision anyway — report it terminal so the route can
            // render 409 instead of recording an orphan decision row.
            return Ok(EscalationLookup::Terminal {
                status: "expired".to_string(),
                at: expires_at,
            });
        }
        Ok(EscalationLookup::Terminal {
            at: decided_at.unwrap_or(expires_at),
            status,
        })
    }

    async fn terminalise(
        &self,
        escalation_id: Uuid,
        verdict: HotlDecisionVerdict,
    ) -> RepoResult<bool> {
        // Deliberately NO `expires_at > ?` guard (contrast
        // `record_decision`): the timeout companion calls this AFTER the
        // deadline passed, so the guard would make the sweep a no-op and
        // the row would stay `pending` forever.
        let result = sqlx::query(
            "UPDATE hotl_pending \
             SET status = ?, decided_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE escalation_id = ? AND status = 'pending'",
        )
        .bind(verdict.status_str())
        .bind(escalation_id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }
}
