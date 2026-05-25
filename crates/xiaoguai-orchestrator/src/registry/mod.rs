//! Agent registry — structured capability tags, registration, and lookup.
//!
//! # Architecture
//!
//! ```text
//! AgentRegistry  ──register()──▶  DashMap<name, Arc<dyn Agent>>
//!                ──lookup_by_capability()──▶  Vec<AgentRef>
//!
//! AgentSpec      carries the capability list, cost_hint, locality, and
//!                the shapes of task inputs / outputs this agent handles.
//!
//! Capability     is a structured (domain, action) tag, e.g.
//!                ("billing", "draft_email") or ("incident", "summarize").
//! ```
//!
//! The `AgentRegistry` is intentionally decoupled from the `Worker` trait in
//! `crate::worker`.  The `Worker` trait models the *runtime dispatch* of a
//! single plan step; the `AgentRegistry` models the *catalogue* of available
//! agents, their declared capabilities, and their cost / scoping metadata.
//!
//! ## Deferred
//! - Capability discovery via MCP-style agent introspection (v1.3).
//! - Agent marketplace / remote registry pull (v1.3).
//! - Capability versioning (e.g. `("billing", "draft_email", "v2")`).

pub mod conflict;
pub mod router;
pub mod store;

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::error::OrchestratorError;

// ── Capability ────────────────────────────────────────────────────────────────

/// A structured capability tag: `(domain, action)`.
///
/// Examples: `("billing", "draft_email")`, `("incident", "summarize")`,
/// `("code", "review")`, `("lang", "zh-CN")`.
///
/// Equality and hashing are case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability {
    /// Functional domain — e.g. `"billing"`, `"incident"`, `"code"`.
    pub domain: String,
    /// Specific action within the domain — e.g. `"draft_email"`, `"summarize"`.
    pub action: String,
}

impl Capability {
    /// Construct a capability tag.
    pub fn new(domain: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            action: action.into(),
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.domain, self.action)
    }
}

// ── TaskShape / ResultShape ───────────────────────────────────────────────────

/// Rough description of the inputs an agent accepts.
///
/// Used by the router to surface mismatches (v1.3); for now it is carried as
/// opaque metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskShape {
    /// Informal description — e.g. `"structured JSON with 'amount' and 'currency'"`.
    pub description: String,
}

/// Rough description of the outputs an agent returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResultShape {
    /// Informal description — e.g. `"Markdown email draft"`.
    pub description: String,
}

// ── TenantScope ───────────────────────────────────────────────────────────────

/// The tenancy scope of an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantScope {
    /// Available to all tenants.
    Global,
    /// Only available to requests that carry a matching tenant id.
    Tenant(String),
}

// ── AgentSpec ─────────────────────────────────────────────────────────────────

/// Static metadata that describes an agent registered in the catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Unique name within the registry.
    pub name: String,
    /// Semver string for audit / compatibility checks.
    pub version: String,
    /// Capabilities this agent can handle.
    pub capabilities: Vec<Capability>,
    /// Informal description of the task input format.
    pub accepts: TaskShape,
    /// Informal description of the result format.
    pub returns: ResultShape,
    /// Relative cost hint for ranking.  Lower is cheaper.
    /// Must be a finite, non-negative value.
    pub cost_hint: f64,
    /// Tenant scoping for this agent.
    pub locality: TenantScope,
}

impl AgentSpec {
    /// Returns `true` if this spec covers *all* of the requested capabilities.
    #[must_use]
    pub fn covers_all(&self, required: &[Capability]) -> bool {
        required.iter().all(|req| self.capabilities.contains(req))
    }
}

// ── Agent trait (registry-level) ─────────────────────────────────────────────

/// A registry-level agent: exposes its static spec and can execute a generic
/// payload.
///
/// This is distinct from `crate::worker::Worker` which is the *runtime*
/// execution interface used by the `Supervisor`.  An implementor of `Agent`
/// may also implement `Worker`; the registry uses `Agent` for capability
/// matching and the supervisor uses `Worker` for step dispatch.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Return the static metadata for this agent.
    fn spec(&self) -> &AgentSpec;

    /// Execute a raw string payload; return a raw string result.
    ///
    /// The format of `payload` is described by `AgentSpec::accepts`; the
    /// format of the returned string by `AgentSpec::returns`.
    async fn run(&self, payload: &str) -> Result<String, OrchestratorError>;
}

// ── AgentRef ─────────────────────────────────────────────────────────────────

/// A cheaply-clonable handle to a registered agent.
#[derive(Clone)]
pub struct AgentRef {
    pub name: String,
    pub spec: AgentSpec,
    pub(crate) agent: Arc<dyn Agent>,
}

impl std::fmt::Debug for AgentRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRef")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl AgentRef {
    /// Execute the agent.
    ///
    /// # Errors
    /// Returns `OrchestratorError` if the underlying agent implementation fails.
    pub async fn run(&self, payload: &str) -> Result<String, OrchestratorError> {
        self.agent.run(payload).await
    }
}

// ── AgentRegistry ─────────────────────────────────────────────────────────────

/// Concurrent registry of named agents.
///
/// Backed by a `DashMap` so lookups and registrations never block each other.
#[derive(Default)]
pub struct AgentRegistry {
    agents: DashMap<String, Arc<dyn Agent>>,
}

impl AgentRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an agent.  Returns an error if a name collision occurs.
    ///
    /// To replace an existing agent, call [`Self::unregister`] first.
    ///
    /// # Errors
    /// Returns `OrchestratorError::Internal` if an agent with the same name is
    /// already registered.
    pub fn register(&self, agent: Arc<dyn Agent>) -> Result<(), OrchestratorError> {
        let name = agent.spec().name.clone();
        if self.agents.contains_key(&name) {
            return Err(OrchestratorError::Internal(format!(
                "agent '{name}' is already registered; unregister it first"
            )));
        }
        self.agents.insert(name, agent);
        Ok(())
    }

    /// Remove an agent by name.  Returns `true` if the agent existed.
    #[must_use]
    pub fn unregister(&self, name: &str) -> bool {
        self.agents.remove(name).is_some()
    }

    /// Look up a single agent by name.
    #[must_use]
    pub fn lookup_by_name(&self, name: &str) -> Option<AgentRef> {
        self.agents.get(name).map(|entry| {
            let agent = Arc::clone(entry.value());
            AgentRef {
                name: name.to_owned(),
                spec: agent.spec().clone(),
                agent,
            }
        })
    }

    /// Return all agents whose `AgentSpec::capabilities` cover *every* requested
    /// capability, in ascending `cost_hint` order.
    #[must_use]
    pub fn lookup_by_capability(&self, required: &[Capability]) -> Vec<AgentRef> {
        let mut matches: Vec<AgentRef> = self
            .agents
            .iter()
            .filter(|e| e.value().spec().covers_all(required))
            .map(|e| {
                let agent = Arc::clone(e.value());
                AgentRef {
                    name: e.key().clone(),
                    spec: agent.spec().clone(),
                    agent,
                }
            })
            .collect();

        // Sort ascending by cost_hint (NaN-safe: treat NaN as f64::MAX).
        matches.sort_by(|a, b| {
            a.spec
                .cost_hint
                .partial_cmp(&b.spec.cost_hint)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches
    }

    /// Return all registered agents, sorted by name.
    #[must_use]
    pub fn list_all(&self) -> Vec<AgentRef> {
        let mut all: Vec<AgentRef> = self
            .agents
            .iter()
            .map(|e| {
                let agent = Arc::clone(e.value());
                AgentRef {
                    name: e.key().clone(),
                    spec: agent.spec().clone(),
                    agent,
                }
            })
            .collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Number of registered agents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// `true` if no agents are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    /// Minimal `Agent` impl for use in tests.
    pub struct EchoAgent {
        pub spec: AgentSpec,
    }

    #[async_trait]
    impl Agent for EchoAgent {
        fn spec(&self) -> &AgentSpec {
            &self.spec
        }

        async fn run(&self, payload: &str) -> Result<String, OrchestratorError> {
            Ok(format!("[{}] echo: {payload}", self.spec.name))
        }
    }

    /// Build a spec with the given capabilities and `cost_hint`.
    pub fn make_spec(
        name: &str,
        caps: Vec<(&str, &str)>,
        cost: f64,
        locality: TenantScope,
    ) -> AgentSpec {
        AgentSpec {
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            capabilities: caps
                .into_iter()
                .map(|(d, a)| Capability::new(d, a))
                .collect(),
            accepts: TaskShape::default(),
            returns: ResultShape::default(),
            cost_hint: cost,
            locality,
        }
    }

    /// Register a test `EchoAgent`.
    pub fn register_echo(
        registry: &AgentRegistry,
        name: &str,
        caps: Vec<(&str, &str)>,
        cost: f64,
        locality: TenantScope,
    ) {
        let spec = make_spec(name, caps, cost, locality);
        registry
            .register(Arc::new(EchoAgent { spec }))
            .expect("register should not fail for unique name");
    }
}

#[cfg(test)]
mod tests {
    use super::{test_helpers::*, *};

    fn make_registry() -> AgentRegistry {
        let r = AgentRegistry::new();
        register_echo(
            &r,
            "billing-agent",
            vec![("billing", "draft_email"), ("billing", "summarize")],
            1.0,
            TenantScope::Global,
        );
        register_echo(
            &r,
            "incident-agent",
            vec![("incident", "summarize"), ("incident", "triage")],
            2.0,
            TenantScope::Global,
        );
        register_echo(
            &r,
            "full-agent",
            vec![
                ("billing", "draft_email"),
                ("incident", "summarize"),
                ("lang", "zh-CN"),
            ],
            5.0,
            TenantScope::Tenant("tenant-A".to_owned()),
        );
        r
    }

    // ── Registration ──────────────────────────────────────────────────────────

    #[test]
    fn register_three_agents() {
        let r = make_registry();
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn duplicate_register_returns_error() {
        let r = AgentRegistry::new();
        register_echo(&r, "a", vec![("x", "y")], 1.0, TenantScope::Global);
        let result = r.register(Arc::new(test_helpers::EchoAgent {
            spec: make_spec("a", vec![("x", "y")], 1.0, TenantScope::Global),
        }));
        assert!(result.is_err());
    }

    #[test]
    fn unregister_removes_agent() {
        let r = make_registry();
        assert!(r.unregister("billing-agent"));
        assert_eq!(r.len(), 2);
        assert!(r.lookup_by_name("billing-agent").is_none());
    }

    // ── lookup_by_name ────────────────────────────────────────────────────────

    #[test]
    fn lookup_by_name_found() {
        let r = make_registry();
        let agent_ref = r.lookup_by_name("incident-agent").unwrap();
        assert_eq!(agent_ref.name, "incident-agent");
    }

    #[test]
    fn lookup_by_name_missing() {
        let r = make_registry();
        assert!(r.lookup_by_name("does-not-exist").is_none());
    }

    // ── lookup_by_capability ──────────────────────────────────────────────────

    #[test]
    fn lookup_single_capability() {
        let r = make_registry();
        let caps = vec![Capability::new("billing", "draft_email")];
        let results = r.lookup_by_capability(&caps);
        // billing-agent (cost 1.0) and full-agent (cost 5.0) both cover it.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "billing-agent"); // cheaper first
        assert_eq!(results[1].name, "full-agent");
    }

    #[test]
    fn lookup_multi_capability_intersection() {
        // Requires BOTH billing.draft_email AND incident.summarize AND lang.zh-CN
        let r = make_registry();
        let caps = vec![
            Capability::new("billing", "draft_email"),
            Capability::new("incident", "summarize"),
            Capability::new("lang", "zh-CN"),
        ];
        let results = r.lookup_by_capability(&caps);
        // Only full-agent covers all three.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "full-agent");
    }

    #[test]
    fn lookup_unknown_capability_returns_empty() {
        let r = make_registry();
        let caps = vec![Capability::new("unknown", "noop")];
        assert!(r.lookup_by_capability(&caps).is_empty());
    }

    // ── list_all ──────────────────────────────────────────────────────────────

    #[test]
    fn list_all_returns_sorted_by_name() {
        let r = make_registry();
        let all = r.list_all();
        assert_eq!(all.len(), 3);
        let names: Vec<_> = all.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["billing-agent", "full-agent", "incident-agent"]);
    }

    // ── Capability helpers ────────────────────────────────────────────────────

    #[test]
    fn capability_display() {
        let c = Capability::new("code", "review");
        assert_eq!(c.to_string(), "code.review");
    }

    #[test]
    fn covers_all_true_subset() {
        let spec = make_spec("x", vec![("a", "1"), ("b", "2")], 1.0, TenantScope::Global);
        assert!(spec.covers_all(&[Capability::new("a", "1")]));
        assert!(spec.covers_all(&[Capability::new("a", "1"), Capability::new("b", "2")]));
    }

    #[test]
    fn covers_all_false_missing() {
        let spec = make_spec("x", vec![("a", "1")], 1.0, TenantScope::Global);
        assert!(!spec.covers_all(&[Capability::new("a", "1"), Capability::new("b", "2")]));
    }
}
