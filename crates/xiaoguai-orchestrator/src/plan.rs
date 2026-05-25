//! `Plan` and `PlanStep` — the unit of work the planner emits.
//!
//! A `PlanStep` carries:
//!   - a unique `id` (used by dependency edges)
//!   - a human-readable `description` (what the worker should do)
//!   - `deps`: ids of steps that must succeed before this step may run
//!   - `status`: current lifecycle state
//!   - `risk_level`: hint to the Supervisor about whether to invoke the Challenger

use serde::{Deserialize, Serialize};

/// Lifecycle state of a single plan step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Waiting for dependencies to complete.
    Pending,
    /// Dispatched to a worker; awaiting result.
    Running,
    /// Worker returned success.
    Succeeded,
    /// Worker returned failure.
    Failed,
    /// Skipped (e.g. dependency never succeeded, or Challenger rejected).
    Skipped,
}

/// Estimated risk level of a plan step.
///
/// The Supervisor routes `High`-risk steps through the `Challenger` before
/// dispatching them to a worker.  `Low` and `Medium` steps bypass the
/// challenger.
///
/// Heuristics for `High` in a default planner:
/// - Writes to external systems (databases, APIs, file-systems)
/// - Financial or payment transactions
/// - Irreversible operations (deletions, sends, publishes)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe, reversible, read-only operations.
    #[default]
    Low,
    /// Operations with moderate impact; typically reversible.
    Medium,
    /// Irreversible, financial, or external-write operations.
    High,
}

/// One unit of work in the plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Unique identifier within this run.
    pub id: String,
    /// Natural-language description of the sub-task.
    pub description: String,
    /// IDs of steps that must appear in the success history before this step
    /// may be dispatched.
    pub deps: Vec<String>,
    /// Current status.
    pub status: StepStatus,
    /// Risk classification — drives challenger routing.  Defaults to `Low`.
    #[serde(default)]
    pub risk_level: RiskLevel,
}

impl PlanStep {
    /// Construct a step in `Pending` state with `RiskLevel::Low`.
    pub fn new(id: impl Into<String>, description: impl Into<String>, deps: Vec<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            deps,
            status: StepStatus::Pending,
            risk_level: RiskLevel::Low,
        }
    }

    /// Builder: set the risk level.
    #[must_use]
    pub fn with_risk(mut self, risk_level: RiskLevel) -> Self {
        self.risk_level = risk_level;
        self
    }

    /// Returns `true` if all `deps` appear in `completed_ids`.
    #[must_use]
    pub fn deps_met(&self, completed_ids: &[String]) -> bool {
        self.deps.iter().all(|d| completed_ids.contains(d))
    }
}

/// An ordered collection of steps. Produced once at the start of a run (for
/// static planners) or built incrementally by the supervisor loop (for dynamic
/// planners). The `Plan` struct is mainly a convenience wrapper used in the
/// example; the supervisor loop itself tracks steps directly.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

impl Plan {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, step: PlanStep) {
        self.steps.push(step);
    }

    /// Returns steps whose deps are all in `completed`.
    #[must_use]
    pub fn ready(&self, completed: &[String]) -> Vec<&PlanStep> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Pending && s.deps_met(completed))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_with_no_deps_always_ready() {
        let s = PlanStep::new("x", "do x", vec![]);
        assert!(s.deps_met(&[]));
        assert!(s.deps_met(&["unrelated".to_string()]));
    }

    #[test]
    fn step_with_dep_blocked_until_met() {
        let s = PlanStep::new("b", "do b", vec!["a".to_string()]);
        assert!(!s.deps_met(&[]));
        assert!(s.deps_met(&["a".to_string()]));
    }

    #[test]
    fn plan_ready_filters_correctly() {
        let mut plan = Plan::new();
        plan.push(PlanStep::new("a", "a", vec![]));
        plan.push(PlanStep::new("b", "b", vec!["a".to_string()]));

        let ready_before: Vec<_> = plan.ready(&[]).iter().map(|s| s.id.clone()).collect();
        assert_eq!(ready_before, vec!["a"]);

        let ready_after: Vec<_> = plan
            .ready(&["a".to_string()])
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(ready_after, vec!["a", "b"]); // a still Pending (no status update in Plan)
    }
}
