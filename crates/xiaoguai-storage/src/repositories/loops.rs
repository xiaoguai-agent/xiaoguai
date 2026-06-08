//! `loops` repository — persistence for /loop session-scoped recurring
//! agent turns (DEC-039 / LLD-LOOP-001 §8).
//!
//! Follows the `hotl_escalations.rs` conventions: a small store trait the
//! api crate consumes (keeping it sqlx-free), plain row types, and a
//! `SqliteLoopRepository` impl with guard-based UPDATEs so state
//! transitions are race-safe at the SQL layer:
//!
//!   - `record_tick` only touches `status = 'active'` rows — a cancel
//!     racing a tick loses cleanly (the tick's bookkeeping is dropped,
//!     the driver sees `false` and stops).
//!   - `terminalise` only moves non-terminal rows (`active`/`paused`) —
//!     a terminal row is immutable, double-cancel returns `false`.
//!
//! The one-live-loop-per-session invariant (v1) is the partial unique
//! index `loops_one_live_per_session`; `insert` surfaces a violation as
//! [`RepoError::DuplicateKey`] for the route to map onto 409.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::repositories::error::{RepoError, RepoResult};

/// Loop lifecycle states. `Active` and `Paused` are live (hold the
/// one-per-session slot); the other four are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStatus {
    Active,
    Paused,
    BudgetExhausted,
    Done,
    Cancelled,
    Failed,
}

impl LoopStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Active | Self::Paused)
    }

    /// Parse the wire/DB string. Returns `None` for unknown values.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "budget_exhausted" => Some(Self::BudgetExhausted),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// How a loop chooses its next-tick delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacingKind {
    /// Fixed `interval_secs` between ticks (L1 default).
    Fixed,
    /// The agent picks each next-tick delay via `loop_next_tick`, clamped to
    /// `[min_interval_secs, max_interval_secs]` (L3 Part B).
    Dynamic,
}

impl PacingKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Dynamic => "dynamic",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fixed" => Some(Self::Fixed),
            "dynamic" => Some(Self::Dynamic),
            _ => None,
        }
    }
}

/// One persisted loop (LLD-LOOP-001 §3).
#[derive(Debug, Clone)]
pub struct LoopRow {
    pub id: Uuid,
    pub session_id: String,
    pub prompt: String,
    pub pacing_kind: PacingKind,
    /// Fixed-pacing interval; also the dynamic-pacing fallback when the
    /// agent doesn't call `loop_next_tick`.
    pub interval_secs: u32,
    /// Dynamic-pacing clamp bounds (unused for `Fixed`).
    pub min_interval_secs: u32,
    pub max_interval_secs: u32,
    pub max_ticks: u32,
    pub ttl_secs: u32,
    /// Token budget (L3 Part C): stop once the session burns this many
    /// tokens since loop-start. `0` = unlimited.
    pub max_total_tokens: u64,
    pub status: LoopStatus,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub next_tick_at: DateTime<Utc>,
    pub ticks_run: u32,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

/// Persistence boundary for loops. The controller and the REST routes
/// consume this trait; production wires [`SqliteLoopRepository`].
#[async_trait]
pub trait LoopStore: Send + Sync {
    /// Insert a new loop. A live loop already holding the session's slot
    /// surfaces as [`RepoError::DuplicateKey`].
    async fn insert(&self, row: &LoopRow) -> RepoResult<()>;

    /// All loops, newest first (terminal rows included — they are the
    /// loop's history).
    async fn list(&self) -> RepoResult<Vec<LoopRow>>;

    async fn get(&self, id: Uuid) -> RepoResult<Option<LoopRow>>;

    /// The session's live (`active`/`paused`) loop, if any — the holder of
    /// the one-per-session slot.
    async fn find_live_by_session(&self, session_id: &str) -> RepoResult<Option<LoopRow>>;

    /// Non-terminal `active` rows for boot replay.
    async fn list_active(&self) -> RepoResult<Vec<LoopRow>>;

    /// Persist one tick's bookkeeping. Returns `false` when the row is no
    /// longer `active` (cancelled/exhausted while the tick ran) — the
    /// driver must stop.
    async fn record_tick(
        &self,
        id: Uuid,
        next_tick_at: DateTime<Utc>,
        ticks_run: u32,
        consecutive_failures: u32,
        last_error: Option<&str>,
    ) -> RepoResult<bool>;

    /// Move a live loop to a terminal `status`. Returns `false` when the
    /// row was already terminal (or missing) — terminal rows are immutable.
    async fn terminalise(
        &self,
        id: Uuid,
        status: LoopStatus,
        reason: Option<&str>,
    ) -> RepoResult<bool>;

    /// Move an `active` loop to `paused` (the agent called `loop_pause`).
    /// Returns `false` when the row is not `active` (already paused/terminal
    /// or missing). `paused` keeps the one-per-session slot — an operator
    /// resumes or cancels it.
    async fn pause(&self, id: Uuid, reason: Option<&str>) -> RepoResult<bool>;
}

/// `SQLite` implementation backed by sqlx.
#[derive(Debug, Clone)]
pub struct SqliteLoopRepository {
    pool: SqlitePool,
}

impl SqliteLoopRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

const LOOP_COLUMNS: &str = "id, session_id, prompt, pacing_kind, interval_secs, \
                            min_interval_secs, max_interval_secs, max_ticks, ttl_secs, \
                            max_total_tokens, status, created_by, created_at, expires_at, \
                            next_tick_at, ticks_run, consecutive_failures, last_error";

/// sqlx row shape; converted into the public [`LoopRow`].
#[derive(sqlx::FromRow)]
struct LoopDbRow {
    id: Uuid,
    session_id: String,
    prompt: String,
    pacing_kind: String,
    interval_secs: i64,
    min_interval_secs: i64,
    max_interval_secs: i64,
    max_ticks: i64,
    ttl_secs: i64,
    max_total_tokens: i64,
    status: String,
    created_by: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    next_tick_at: DateTime<Utc>,
    ticks_run: i64,
    consecutive_failures: i64,
    last_error: Option<String>,
}

impl TryFrom<LoopDbRow> for LoopRow {
    type Error = RepoError;

    fn try_from(r: LoopDbRow) -> Result<Self, RepoError> {
        let status = LoopStatus::parse(&r.status).ok_or_else(|| {
            RepoError::InvalidArgument(format!("unknown loop status in DB: {}", r.status))
        })?;
        let pacing_kind = PacingKind::parse(&r.pacing_kind).ok_or_else(|| {
            RepoError::InvalidArgument(format!("unknown pacing_kind in DB: {}", r.pacing_kind))
        })?;
        Ok(Self {
            id: r.id,
            session_id: r.session_id,
            prompt: r.prompt,
            pacing_kind,
            interval_secs: clamp_u32(r.interval_secs),
            min_interval_secs: clamp_u32(r.min_interval_secs),
            max_interval_secs: clamp_u32(r.max_interval_secs),
            max_ticks: clamp_u32(r.max_ticks),
            ttl_secs: clamp_u32(r.ttl_secs),
            max_total_tokens: u64::try_from(r.max_total_tokens.max(0)).unwrap_or(u64::MAX),
            status,
            created_by: r.created_by,
            created_at: r.created_at,
            expires_at: r.expires_at,
            next_tick_at: r.next_tick_at,
            ticks_run: clamp_u32(r.ticks_run),
            consecutive_failures: clamp_u32(r.consecutive_failures),
            last_error: r.last_error,
        })
    }
}

fn clamp_u32(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}

#[async_trait]
impl LoopStore for SqliteLoopRepository {
    async fn insert(&self, row: &LoopRow) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO loops \
             (id, session_id, prompt, pacing_kind, interval_secs, min_interval_secs, \
              max_interval_secs, max_ticks, ttl_secs, max_total_tokens, status, \
              created_by, created_at, expires_at, next_tick_at, ticks_run, \
              consecutive_failures, last_error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(&row.session_id)
        .bind(&row.prompt)
        .bind(row.pacing_kind.as_str())
        .bind(i64::from(row.interval_secs))
        .bind(i64::from(row.min_interval_secs))
        .bind(i64::from(row.max_interval_secs))
        .bind(i64::from(row.max_ticks))
        .bind(i64::from(row.ttl_secs))
        .bind(i64::try_from(row.max_total_tokens).unwrap_or(i64::MAX))
        .bind(row.status.as_str())
        .bind(&row.created_by)
        .bind(row.created_at)
        .bind(row.expires_at)
        .bind(row.next_tick_at)
        .bind(i64::from(row.ticks_run))
        .bind(i64::from(row.consecutive_failures))
        .bind(row.last_error.as_deref())
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn list(&self) -> RepoResult<Vec<LoopRow>> {
        let rows: Vec<LoopDbRow> = sqlx::query_as(&format!(
            "SELECT {LOOP_COLUMNS} FROM loops ORDER BY created_at DESC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(LoopRow::try_from).collect()
    }

    async fn get(&self, id: Uuid) -> RepoResult<Option<LoopRow>> {
        let row: Option<LoopDbRow> =
            sqlx::query_as(&format!("SELECT {LOOP_COLUMNS} FROM loops WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(RepoError::from_sqlx)?;
        row.map(LoopRow::try_from).transpose()
    }

    async fn find_live_by_session(&self, session_id: &str) -> RepoResult<Option<LoopRow>> {
        let row: Option<LoopDbRow> = sqlx::query_as(&format!(
            "SELECT {LOOP_COLUMNS} FROM loops \
             WHERE session_id = ? AND status IN ('active', 'paused')"
        ))
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        row.map(LoopRow::try_from).transpose()
    }

    async fn list_active(&self) -> RepoResult<Vec<LoopRow>> {
        let rows: Vec<LoopDbRow> = sqlx::query_as(&format!(
            "SELECT {LOOP_COLUMNS} FROM loops WHERE status = 'active' \
             ORDER BY next_tick_at ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(LoopRow::try_from).collect()
    }

    async fn record_tick(
        &self,
        id: Uuid,
        next_tick_at: DateTime<Utc>,
        ticks_run: u32,
        consecutive_failures: u32,
        last_error: Option<&str>,
    ) -> RepoResult<bool> {
        // `status = 'active'` guard: a loop cancelled (or exhausted) while
        // the tick ran must NOT be revived by late bookkeeping.
        let result = sqlx::query(
            "UPDATE loops \
             SET next_tick_at = ?, ticks_run = ?, consecutive_failures = ?, \
                 last_error = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ? AND status = 'active'",
        )
        .bind(next_tick_at)
        .bind(i64::from(ticks_run))
        .bind(i64::from(consecutive_failures))
        .bind(last_error)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }

    async fn terminalise(
        &self,
        id: Uuid,
        status: LoopStatus,
        reason: Option<&str>,
    ) -> RepoResult<bool> {
        if !status.is_terminal() {
            return Err(RepoError::InvalidArgument(format!(
                "terminalise requires a terminal status, got {}",
                status.as_str()
            )));
        }
        let result = sqlx::query(
            "UPDATE loops \
             SET status = ?, last_error = COALESCE(?, last_error), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ? AND status IN ('active', 'paused')",
        )
        .bind(status.as_str())
        .bind(reason)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }

    async fn pause(&self, id: Uuid, reason: Option<&str>) -> RepoResult<bool> {
        let result = sqlx::query(
            "UPDATE loops \
             SET status = 'paused', last_error = COALESCE(?, last_error), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE id = ? AND status = 'active'",
        )
        .bind(reason)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(result.rows_affected() > 0)
    }
}
