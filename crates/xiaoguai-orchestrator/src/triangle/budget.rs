//! `TriangleBudget` — per-role budget split for the triangle pattern.
//! DEC-021 §4.6: default 50 % Worker / 40 % Planner / 10 % Critic.
//!
//! Critic's small share is by design — it makes accept/reject
//! decisions, not artefacts. If a deployment finds the Critic over-
//! budget regularly, the right answer is usually to shrink the
//! rubric in `AcceptanceCriteria`, not to bump the Critic budget.

use serde::{Deserialize, Serialize};

/// Per-role budget split. Percentages MUST sum to 100; constructed
/// values are validated by `TriangleBudget::new` and round-trip
/// through serde without revalidation (so deserialised invalid
/// budgets would be accepted by serde — callers MUST re-validate
/// after deserialisation in production code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriangleBudget {
    pub worker_pct: u32,
    pub planner_pct: u32,
    pub critic_pct: u32,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BudgetError {
    #[error("budget percentages must sum to 100; got {0}")]
    PercentagesDontSumTo100(u32),
    #[error(
        "parent budget {parent} too small for split {worker_pct}/{planner_pct}/{critic_pct} — \
         smallest per-role cap would be 0 tokens; minimum parent budget is {min}"
    )]
    BudgetTooSmall {
        parent: u64,
        worker_pct: u32,
        planner_pct: u32,
        critic_pct: u32,
        min: u64,
    },
}

impl TriangleBudget {
    pub const DEFAULT: Self = Self {
        worker_pct: 50,
        planner_pct: 40,
        critic_pct: 10,
    };

    /// Construct + validate percentages-sum-to-100. Use this in
    /// production paths; the public field constructor is available
    /// only for tests and serde.
    ///
    /// # Errors
    /// `PercentagesDontSumTo100` if the three percentages don't
    /// total 100 exactly.
    pub fn new(worker_pct: u32, planner_pct: u32, critic_pct: u32) -> Result<Self, BudgetError> {
        let sum = worker_pct + planner_pct + critic_pct;
        if sum != 100 {
            return Err(BudgetError::PercentagesDontSumTo100(sum));
        }
        Ok(Self {
            worker_pct,
            planner_pct,
            critic_pct,
        })
    }

    /// Split a parent token budget into per-role caps. Refuses the
    /// split if any per-role cap would be 0 (`BudgetTooSmall`) — this
    /// is the §4 risk-row mitigation, fail-early before the Planner
    /// is even spawned.
    ///
    /// # Errors
    /// `BudgetTooSmall` if the smallest percentage applied to
    /// `parent_budget` would floor to zero.
    pub fn split(self, parent_budget: u64) -> Result<RoleBudgets, BudgetError> {
        let worker = parent_budget * u64::from(self.worker_pct) / 100;
        let planner = parent_budget * u64::from(self.planner_pct) / 100;
        let critic = parent_budget * u64::from(self.critic_pct) / 100;

        if worker == 0 || planner == 0 || critic == 0 {
            // Compute the minimum parent budget that would give every
            // role at least 1 token: solve `min * smallest_pct / 100 >= 1`.
            let smallest_pct = self
                .worker_pct
                .min(self.planner_pct)
                .min(self.critic_pct);
            let min = if smallest_pct == 0 {
                u64::MAX
            } else {
                100u64.div_ceil(u64::from(smallest_pct))
            };
            return Err(BudgetError::BudgetTooSmall {
                parent: parent_budget,
                worker_pct: self.worker_pct,
                planner_pct: self.planner_pct,
                critic_pct: self.critic_pct,
                min,
            });
        }

        Ok(RoleBudgets {
            worker,
            planner,
            critic,
        })
    }
}

impl Default for TriangleBudget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Result of `TriangleBudget::split` — per-role token caps. The
/// orchestrator wraps each spawned `AgentLoop` with a `BudgetEnforcer`
/// (implementation lands in S9-5) that aborts when the corresponding
/// cap is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleBudgets {
    pub worker: u64,
    pub planner: u64,
    pub critic: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_50_40_10() {
        let b = TriangleBudget::DEFAULT;
        assert_eq!(b.worker_pct, 50);
        assert_eq!(b.planner_pct, 40);
        assert_eq!(b.critic_pct, 10);
    }

    #[test]
    fn percentages_must_sum_to_100() {
        let err = TriangleBudget::new(40, 40, 10).unwrap_err();
        assert_eq!(err, BudgetError::PercentagesDontSumTo100(90));
    }

    #[test]
    fn happy_split_at_1000_tokens() {
        let b = TriangleBudget::DEFAULT;
        let caps = b.split(1000).unwrap();
        assert_eq!(caps.worker, 500);
        assert_eq!(caps.planner, 400);
        assert_eq!(caps.critic, 100);
    }

    #[test]
    fn budget_too_small_rejected() {
        // 10 tokens, 10% to Critic → 1 token cap; OK.
        let _ = TriangleBudget::DEFAULT.split(10).unwrap();
        // 9 tokens, 10% to Critic → 0 token cap; reject.
        let err = TriangleBudget::DEFAULT.split(9).unwrap_err();
        match err {
            BudgetError::BudgetTooSmall { parent, min, .. } => {
                assert_eq!(parent, 9);
                assert_eq!(min, 10); // need parent ≥ 10 for 10% to be ≥ 1
            }
            _ => panic!("expected BudgetTooSmall"),
        }
    }

    #[test]
    fn budget_round_trips_through_serde() {
        let b = TriangleBudget::DEFAULT;
        let json = serde_json::to_string(&b).unwrap();
        let back: TriangleBudget = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn alternative_splits_validate() {
        // Slow-burn experiment: 60/30/10 (less Worker, more Critic
        // weight than DEFAULT — operator might pick this for high-
        // quality regulated workflows).
        TriangleBudget::new(60, 30, 10).unwrap();
        // 33/33/34 → invalid (sum 100 but Critic > Planner reads
        // wrong; validator only checks sum, not ordering, so OK).
        TriangleBudget::new(33, 33, 34).unwrap();
    }
}
