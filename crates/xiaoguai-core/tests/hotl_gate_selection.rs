//! Sprint-12 S12-4: integration test for the `run_serve` gate-selection
//! helper. Proves that `agent.hotl.suspend_on_escalate=false` (the v1.8.x
//! default) keeps the legacy `EnforcerGate` semantics (Escalate → Allow +
//! warn), while `true` swaps in `SuspendingHotlGate` (Escalate →
//! Suspend{ticket}). This is the "Default-off proof" required by plan
//! §3.2 for any PR that wires the new suspend path into `run_serve`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult};
use xiaoguai_core::hotl_bridge::build_hotl_gate;

/// Always returns `Escalate(reason)` — the only verdict where the two
/// adapters diverge.
#[derive(Debug)]
struct AlwaysEscalate;

#[async_trait]
impl HotlEnforcer for AlwaysEscalate {
    async fn check(&self, _tenant: Uuid, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(HotlVerdict::Escalate("test escalate".into()))
    }
}

#[tokio::test]
async fn default_off_keeps_enforcer_gate_behaviour() {
    // suspend_on_escalate = false → EnforcerGate semantics. Escalate folds
    // into Allow + tracing::warn. No ticket is registered.
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let gate = build_hotl_gate(
        false,
        enforcer,
        registry.clone(),
        Duration::from_secs(24 * 3600),
    );

    let verdict = gate.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
    assert!(
        matches!(verdict, xiaoguai_agent::HotlGateVerdict::Allow),
        "v1.8.x default (suspend_on_escalate=false) must keep Escalate→Allow; got {verdict:?}"
    );
    assert!(
        registry.is_empty(),
        "legacy EnforcerGate must NOT register a waiter on the shared registry"
    );
}

#[tokio::test]
async fn opt_in_swaps_in_suspending_gate() {
    // suspend_on_escalate = true → SuspendingHotlGate. Escalate mints a
    // ticket and returns Suspend; the registry holds one waiter until the
    // ticket is awaited or resolved.
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let gate = build_hotl_gate(
        true,
        enforcer,
        registry.clone(),
        Duration::from_secs(24 * 3600),
    );

    let verdict = gate.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
    assert!(
        matches!(verdict, xiaoguai_agent::HotlGateVerdict::Suspend { .. }),
        "opt-in (suspend_on_escalate=true) must swap in SuspendingHotlGate; got {verdict:?}"
    );
    assert_eq!(
        registry.len(),
        1,
        "SuspendingHotlGate must register exactly one waiter on the shared registry"
    );
}
