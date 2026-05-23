//! Per-user proactive-push budget — the spam guardrail.
//!
//! Roadmap §5.5 is explicit: **3 proactive pushes per user per day by
//! default, configurable.** Sinks may refuse delivery if the reason
//! field is empty. The ledger lives in this crate so the runner can
//! consult it before firing a [`crate::trigger::Trigger::Proactive`]
//! job, and so a tenant exhausting their budget produces an audit row
//! ("denied: out of budget") just like any other scheduler outcome.
//!
//! The trait is intentionally tiny:
//!
//! * [`BudgetLedger::check_and_debit`] atomically asks "does this user
//!   have budget left today?" and if so debits one slot. Returns `true`
//!   when the slot was claimed, `false` when the user is out.
//! * [`BudgetLedger::remaining`] is read-only — useful for diagnostics
//!   and admin-ui dashboards.
//!
//! Day boundary uses `NaiveDate` (UTC). Tenants who care about local
//! "day" semantics can shift the budget reset by configuring the runner
//! with a tenant-aware ledger later — out of scope for v0.10.2.
//!
//! v0.10.2 ships the in-memory impl; a PG-backed ledger lands in
//! v0.12.0 together with the rest of the scheduler PG repos.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::NaiveDate;
use parking_lot::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BudgetError {
    #[error("ledger backend: {0}")]
    Backend(String),
}

/// Default budget per user per day (roadmap §5.5 — non-negotiable
/// default; operators can raise via [`crate::runner::RunnerOptions`]).
pub const DEFAULT_PROACTIVE_BUDGET_PER_DAY: u32 = 3;

#[async_trait]
pub trait BudgetLedger: Send + Sync {
    /// Atomic "check, and if there's room, debit one slot" for
    /// `user_id` on `day`. Returns `true` when a slot was claimed,
    /// `false` when the user has already hit the cap for the day.
    async fn check_and_debit(&self, user_id: &str, day: NaiveDate) -> Result<bool, BudgetError>;

    /// Remaining slots for `user_id` on `day`. Read-only.
    async fn remaining(&self, user_id: &str, day: NaiveDate) -> Result<u32, BudgetError>;
}

/// In-memory ledger keyed on `(user_id, day)`. Reset is implicit: the
/// previous day's bucket simply isn't read again, so memory grows in
/// proportion to "unique users × days the process has been up". Fine
/// for the v0.10.2 in-process scheduler; the PG-backed impl in v0.12.0
/// stores rows that expire by housekeeping.
pub struct InMemoryBudgetLedger {
    limit_per_day: u32,
    used: Mutex<HashMap<(String, NaiveDate), u32>>,
}

impl InMemoryBudgetLedger {
    #[must_use]
    pub fn new(limit_per_day: u32) -> Self {
        Self {
            limit_per_day,
            used: Mutex::new(HashMap::new()),
        }
    }

    /// Construct with the roadmap-default 3/day cap.
    #[must_use]
    pub fn with_default_limit() -> Self {
        Self::new(DEFAULT_PROACTIVE_BUDGET_PER_DAY)
    }

    #[must_use]
    pub fn limit(&self) -> u32 {
        self.limit_per_day
    }
}

impl Default for InMemoryBudgetLedger {
    fn default() -> Self {
        Self::with_default_limit()
    }
}

impl std::fmt::Debug for InMemoryBudgetLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryBudgetLedger")
            .field("limit_per_day", &self.limit_per_day)
            .field("tracked_buckets", &self.used.lock().len())
            .finish()
    }
}

#[async_trait]
impl BudgetLedger for InMemoryBudgetLedger {
    async fn check_and_debit(&self, user_id: &str, day: NaiveDate) -> Result<bool, BudgetError> {
        let mut g = self.used.lock();
        let entry = g.entry((user_id.to_string(), day)).or_insert(0);
        if *entry >= self.limit_per_day {
            return Ok(false);
        }
        *entry += 1;
        Ok(true)
    }

    async fn remaining(&self, user_id: &str, day: NaiveDate) -> Result<u32, BudgetError> {
        let g = self.used.lock();
        let used = g.get(&(user_id.to_string(), day)).copied().unwrap_or(0);
        Ok(self.limit_per_day.saturating_sub(used))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        chrono::Utc
            .with_ymd_and_hms(y, m, d, 0, 0, 0)
            .unwrap()
            .date_naive()
    }

    #[tokio::test]
    async fn debits_until_limit() {
        let ledger = InMemoryBudgetLedger::new(3);
        let d = day(2026, 5, 23);
        for _ in 0..3 {
            assert!(ledger.check_and_debit("u1", d).await.unwrap());
        }
        assert!(!ledger.check_and_debit("u1", d).await.unwrap());
        assert_eq!(ledger.remaining("u1", d).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn different_users_have_independent_budgets() {
        let ledger = InMemoryBudgetLedger::new(2);
        let d = day(2026, 5, 23);
        assert!(ledger.check_and_debit("u1", d).await.unwrap());
        assert!(ledger.check_and_debit("u1", d).await.unwrap());
        assert!(!ledger.check_and_debit("u1", d).await.unwrap());
        assert!(ledger.check_and_debit("u2", d).await.unwrap());
        assert_eq!(ledger.remaining("u2", d).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn different_days_have_independent_budgets() {
        let ledger = InMemoryBudgetLedger::new(1);
        assert!(ledger
            .check_and_debit("u1", day(2026, 5, 23))
            .await
            .unwrap());
        assert!(!ledger
            .check_and_debit("u1", day(2026, 5, 23))
            .await
            .unwrap());
        assert!(ledger
            .check_and_debit("u1", day(2026, 5, 24))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn default_limit_matches_roadmap() {
        let ledger = InMemoryBudgetLedger::default();
        assert_eq!(ledger.limit(), DEFAULT_PROACTIVE_BUDGET_PER_DAY);
        assert_eq!(ledger.limit(), 3);
    }

    #[tokio::test]
    async fn remaining_starts_at_limit() {
        let ledger = InMemoryBudgetLedger::new(5);
        assert_eq!(
            ledger
                .remaining("never-seen", day(2026, 5, 23))
                .await
                .unwrap(),
            5
        );
    }
}
