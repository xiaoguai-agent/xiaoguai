//! `Verdict` — Critic's output per `WorkerResult`. Three outcomes:
//!
//! - `Approve(reason)` — Worker's artefact passes the rubric;
//!   orchestrator promotes Scratchpad → session memory.
//! - `RequestRevision(feedback)` — Worker re-runs the same task with
//!   `feedback` injected as additional context. Capped per task by
//!   `max_revisions` (default 3 from DEC-021 §4.7).
//! - `Reject(reason)` — Task is fundamentally wrong; hand back to
//!   Planner. Counts toward the replan cap.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    Approve { reason: String },
    RequestRevision { feedback: String },
    Reject { reason: String },
}

/// Coarse classification used by counters and dashboards. Stable
/// labels — must NOT change between versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Approve,
    RequestRevision,
    Reject,
}

impl VerdictKind {
    /// Stable metric label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestRevision => "request_revision",
            Self::Reject => "reject",
        }
    }
}

impl Verdict {
    #[must_use]
    pub fn kind(&self) -> VerdictKind {
        match self {
            Self::Approve { .. } => VerdictKind::Approve,
            Self::RequestRevision { .. } => VerdictKind::RequestRevision,
            Self::Reject { .. } => VerdictKind::Reject,
        }
    }

    /// Human-readable explanation (the `reason` or `feedback`
    /// string). Surfaced to the LLM on revision or to the operator
    /// in the final summary.
    #[must_use]
    pub fn explanation(&self) -> &str {
        match self {
            Self::Approve { reason } | Self::Reject { reason } => reason.as_str(),
            Self::RequestRevision { feedback } => feedback.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_round_trips_through_serde() {
        for v in [
            Verdict::Approve {
                reason: "ok".into(),
            },
            Verdict::RequestRevision {
                feedback: "add citation".into(),
            },
            Verdict::Reject {
                reason: "wrong task".into(),
            },
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: Verdict = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn verdict_kind_labels_are_stable() {
        assert_eq!(VerdictKind::Approve.as_str(), "approve");
        assert_eq!(VerdictKind::RequestRevision.as_str(), "request_revision");
        assert_eq!(VerdictKind::Reject.as_str(), "reject");
    }

    #[test]
    fn verdict_kind_maps_consistently() {
        assert_eq!(
            Verdict::Approve { reason: "".into() }.kind(),
            VerdictKind::Approve
        );
        assert_eq!(
            Verdict::RequestRevision {
                feedback: "".into()
            }
            .kind(),
            VerdictKind::RequestRevision
        );
        assert_eq!(
            Verdict::Reject { reason: "".into() }.kind(),
            VerdictKind::Reject
        );
    }

    #[test]
    fn verdict_explanation_returns_inner_string() {
        assert_eq!(
            Verdict::Approve {
                reason: "looks good".into()
            }
            .explanation(),
            "looks good"
        );
        assert_eq!(
            Verdict::RequestRevision {
                feedback: "missing source".into()
            }
            .explanation(),
            "missing source"
        );
        assert_eq!(
            Verdict::Reject {
                reason: "off-topic".into()
            }
            .explanation(),
            "off-topic"
        );
    }
}
