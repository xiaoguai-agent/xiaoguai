//! Sprint-12 S12-12 — default-on proof for v1.9.0.
//!
//! This test pins the v1.9.0 user-visible behaviour change: a fresh
//! deployment (no explicit `agent.hotl.suspend_on_escalate` in
//! `config.yaml`, no env override) must default to **suspension**, i.e.
//! `Settings::default().agent.hotl.suspend_on_escalate == true`, and
//! the gate selector in `run_serve` (`build_hotl_gate`) must therefore
//! select `SuspendingHotlGate` — NOT the legacy `EnforcerGate`.
//!
//! This is the §3.2 behaviour-gate verification for the default flip.
//! It is the counterpart of:
//!
//! - `crates/xiaoguai-core/tests/hotl_gate_selection.rs` (S12-4) which
//!   pins both branches of `build_hotl_gate(bool, ...)` independent of
//!   the default;
//! - `crates/xiaoguai-agent/tests/hotl_legacy_no_suspend.rs` (S12-5/9)
//!   which pins that explicit opt-out (`suspend_on_escalate=false`)
//!   continues to emit zero `HotL` events through the ReAct loop.
//!
//! Together the three tests prove: v1.9.0 default = suspension;
//! v1.8.x semantics still available via explicit opt-out.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult};
use xiaoguai_config::Settings;
use xiaoguai_core::hotl_bridge::build_hotl_gate;

/// Always returns `Escalate(reason)` — the verdict whose handling
/// differs between `EnforcerGate` (legacy: Allow + warn) and
/// `SuspendingHotlGate` (v1.9.0 default: mint ticket + Suspend).
#[derive(Debug)]
struct AlwaysEscalate;

#[async_trait]
impl HotlEnforcer for AlwaysEscalate {
    async fn check(&self, _tenant: Uuid, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(HotlVerdict::Escalate("test escalate".into()))
    }
}

/// `Settings::default()` (used both as the in-memory default base and
/// as the YAML serialised under `load_from_env`) must return
/// `suspend_on_escalate == true` in v1.9.0.
#[test]
fn default_settings_have_suspend_on_escalate_true() {
    let s = Settings::default();
    assert!(
        s.agent.hotl.suspend_on_escalate,
        "v1.9.0 default must be true (S12-12); got false (still on v1.8.x default)"
    );
}

/// A config.yaml omitting the entire `agent` block (the path most
/// tenants take after upgrading from v1.8.x without touching their
/// config) must also resolve to `suspend_on_escalate == true`.
#[test]
fn default_yaml_load_path_has_suspend_on_escalate_true() {
    let s = Settings::load_from_env().expect("default env load");
    assert!(
        s.agent.hotl.suspend_on_escalate,
        "v1.9.0 default must be true via env loader path; got false"
    );
}

/// End-to-end: with the v1.9.0 default in place, the gate selector
/// inside `run_serve` (`build_hotl_gate(settings.agent.hotl.suspend_on_escalate, ...)`)
/// must hand back a `SuspendingHotlGate` — `Escalate` mints a ticket
/// and returns `Suspend`. Mirrors the production wiring path.
#[tokio::test]
async fn default_run_serve_selects_suspending_gate() {
    let settings = Settings::default();
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let gate = build_hotl_gate(
        settings.agent.hotl.suspend_on_escalate,
        enforcer,
        registry.clone(),
        Duration::from_secs(24 * 3600),
    );

    let verdict = gate.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
    assert!(
        matches!(verdict, xiaoguai_agent::HotlGateVerdict::Suspend { .. }),
        "v1.9.0 default config must wire SuspendingHotlGate; got {verdict:?}"
    );
    assert_eq!(
        registry.len(),
        1,
        "SuspendingHotlGate must register exactly one waiter on the shared registry"
    );
}
