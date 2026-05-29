//! Triangle roles. Stable labels — used in metrics, audit, and
//! dashboards; MUST NOT change between versions.

use serde::{Deserialize, Serialize};

/// One of the three triangle roles (DEC-021). The role is a *runtime*
/// designation set by the orchestrator when it picks a persona for a
/// pattern slot — personas themselves remain unrestricted templates,
/// so a persona registered as a Critic in one invocation can be used
/// as a Worker in another. See `lld-personas.md` v1.6+ note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Decomposes the goal into a `Plan` of `Task`s.
    Planner,
    /// Executes one `Task` as a full ReAct loop, writing to a private
    /// `Scratchpad`.
    Worker,
    /// Reviews each `WorkerResult` and emits a `Verdict`.
    Critic,
}

impl Role {
    /// Stable label for metrics + audit. Must NOT change between
    /// versions — operators key dashboards off it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Worker => "worker",
            Self::Critic => "critic",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_labels_are_stable() {
        // Dashboards key on these strings; pin them.
        assert_eq!(Role::Planner.as_str(), "planner");
        assert_eq!(Role::Worker.as_str(), "worker");
        assert_eq!(Role::Critic.as_str(), "critic");
    }

    #[test]
    fn role_round_trips_through_serde() {
        for r in [Role::Planner, Role::Worker, Role::Critic] {
            let json = serde_json::to_string(&r).unwrap();
            let back: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(r, back);
        }
    }
}
