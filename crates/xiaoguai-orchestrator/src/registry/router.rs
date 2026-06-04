//! Capability-based router.
//!
//! Given an `Intent` (a goal + a set of required capabilities), the router
//! queries the `AgentRegistry`, ranks matching agents, and returns a
//! `Dispatch` struct carrying the primary agent and ordered fallbacks.
//!
//! # Ranking rules (applied in order, all tie-broken by the next rule)
//!
//! 1. **Cost hint** — lower `cost_hint` wins.
//! 2. **Round-robin** — among equally-ranked agents the router picks the next
//!    one via an atomic counter so no single agent monopolises load.
//!
//! # Deferred
//! - Soft capability matching (e.g. semantic similarity scores).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::error::OrchestratorError;

use super::{AgentRef, AgentRegistry, Capability};

// ── Intent ────────────────────────────────────────────────────────────────────

/// Inbound routing request.
#[derive(Debug, Clone)]
pub struct Intent {
    /// Human-readable goal (passed through for logging/tracing).
    pub goal: String,
    /// Capabilities that the selected agent *must* cover.
    pub required_capabilities: Vec<Capability>,
}

impl Intent {
    #[must_use]
    pub fn new(goal: impl Into<String>, required_capabilities: Vec<Capability>) -> Self {
        Self {
            goal: goal.into(),
            required_capabilities,
        }
    }
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// The router's answer to an `Intent`.
#[derive(Debug)]
pub struct Dispatch {
    /// The top-ranked agent to use.
    pub primary: AgentRef,
    /// Fallback agents in ranked order (excludes `primary`).
    pub fallbacks: Vec<AgentRef>,
}

// ── CapabilityRouter ─────────────────────────────────────────────────────────

/// Routes intents to agents based on capability coverage and cost.
pub struct CapabilityRouter {
    registry: Arc<AgentRegistry>,
    /// Round-robin counter — bumped every time a tie-break occurs.
    rr_counter: AtomicUsize,
}

impl CapabilityRouter {
    #[must_use]
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            rr_counter: AtomicUsize::new(0),
        }
    }

    /// Route the intent to the best available agent.
    ///
    /// # Errors
    /// Returns `Err(OrchestratorError::Internal)` if no agent covers the
    /// required capabilities.
    pub fn route(&self, intent: &Intent) -> Result<Dispatch, OrchestratorError> {
        let mut candidates = self
            .registry
            .lookup_by_capability(&intent.required_capabilities);

        if candidates.is_empty() {
            return Err(OrchestratorError::Internal(format!(
                "no agent covers the required capabilities: {:?}",
                intent
                    .required_capabilities
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        // Apply ranking.
        self.rank(&mut candidates);

        let primary = candidates.remove(0);
        Ok(Dispatch {
            primary,
            fallbacks: candidates,
        })
    }

    /// Sort `candidates` in-place according to the ranking rules.
    ///
    /// Sort key per candidate (lexicographically ascending = higher priority first):
    ///   1. `cost_hint` ascending (f64 bits, NaN → MAX)
    ///   2. round-robin position (rotated by `rr_counter`)
    fn rank(&self, candidates: &mut Vec<AgentRef>) {
        let rr = self.rr_counter.fetch_add(1, Ordering::Relaxed);
        let n = candidates.len();

        // Pre-compute stable alphabetical positions for round-robin.
        let mut sorted_names: Vec<&str> = candidates.iter().map(|a| a.name.as_str()).collect();
        sorted_names.sort_unstable();

        // Build a sort key for each candidate; keep the index stable.
        let keys: Vec<(u64, usize)> = candidates
            .iter()
            .map(|a| {
                // Cost: convert to sortable bits (NaN → MAX).
                let cost_bits = if a.spec.cost_hint.is_nan() {
                    u64::MAX
                } else {
                    a.spec.cost_hint.to_bits()
                };
                // Round-robin position within alphabetical order.
                let pos = sorted_names
                    .iter()
                    .position(|&s| s == a.name.as_str())
                    .unwrap_or(0);
                let rr_pos = if n == 0 { 0 } else { (pos + n - (rr % n)) % n };
                (cost_bits, rr_pos)
            })
            .collect();

        // Sort by the pre-computed key; indices into `keys` match `candidates`.
        let mut indices: Vec<usize> = (0..n).collect();
        indices.sort_by_key(|&i| keys[i]);
        let sorted: Vec<AgentRef> = indices.into_iter().map(|i| candidates[i].clone()).collect();
        *candidates = sorted;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{
        test_helpers::{make_spec, EchoAgent},
        AgentRegistry,
    };

    fn make_router() -> (Arc<AgentRegistry>, CapabilityRouter) {
        let registry = Arc::new(AgentRegistry::new());

        // agent-cheap: billing only, cost 1.0
        registry
            .register(Arc::new(EchoAgent {
                spec: make_spec("agent-cheap", vec![("billing", "draft_email")], 1.0),
            }))
            .unwrap();

        // agent-mid: billing + lang.zh-CN, cost 2.0
        registry
            .register(Arc::new(EchoAgent {
                spec: make_spec(
                    "agent-mid",
                    vec![("billing", "draft_email"), ("lang", "zh-CN")],
                    2.0,
                ),
            }))
            .unwrap();

        // agent-pricey: billing + lang.zh-CN, cost 3.0
        registry
            .register(Arc::new(EchoAgent {
                spec: make_spec(
                    "agent-pricey",
                    vec![("billing", "draft_email"), ("lang", "zh-CN")],
                    3.0,
                ),
            }))
            .unwrap();

        let router = CapabilityRouter::new(Arc::clone(&registry));
        (registry, router)
    }

    // ── Basic routing ─────────────────────────────────────────────────────────

    #[test]
    fn route_single_capability_picks_cheapest() {
        let (_r, router) = make_router();
        let intent = Intent::new(
            "draft email",
            vec![Capability::new("billing", "draft_email")],
        );
        let dispatch = router.route(&intent).unwrap();
        assert_eq!(dispatch.primary.name, "agent-cheap");
    }

    #[test]
    fn route_required_two_caps_picks_covering_agent() {
        let (_r, router) = make_router();
        let intent = Intent::new(
            "draft email in Chinese",
            vec![
                Capability::new("billing", "draft_email"),
                Capability::new("lang", "zh-CN"),
            ],
        );
        let dispatch = router.route(&intent).unwrap();
        // agent-cheap does NOT cover lang.zh-CN → not in candidates.
        // agent-mid (cost 2.0) should beat agent-pricey (cost 3.0).
        assert_eq!(dispatch.primary.name, "agent-mid");
    }

    #[test]
    fn route_unknown_capability_returns_error() {
        let (_r, router) = make_router();
        let intent = Intent::new("noop", vec![Capability::new("unknown", "noop")]);
        assert!(router.route(&intent).is_err());
    }

    #[test]
    fn dispatch_includes_all_fallbacks() {
        let (_r, router) = make_router();
        let intent = Intent::new(
            "draft email",
            vec![Capability::new("billing", "draft_email")],
        );
        let dispatch = router.route(&intent).unwrap();
        // All 3 agents cover billing.draft_email; primary + 2 fallbacks.
        assert_eq!(dispatch.fallbacks.len(), 2);
    }
}
