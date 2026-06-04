//! Sprint-13 S13-7 — per-scope `HotL` expiry lookup in `SuspendingHotlGate`.
//!
//! Pins the v1.10 behaviour: when the gate maps an upstream `Escalate`
//! verdict to `HotlGateVerdict::Suspend`, the minted ticket's `expires_at`
//! is computed from a per-scope expiry table keyed on the **scope class**
//! (the prefix before the first `.`). Missing-class lookups fall back to
//! the gate's `default_expiry`.
//!
//! Wave-1 S13-0 (PR #139) already landed the config surface
//! (`HotlSettings::expiry: HashMap<String, Duration>`); this test pins
//! the wiring from that config field through `SuspendingHotlGate::new`
//! into the ticket emitted on the suspend path.
//!
//! Lookup is per-call (no caching), so tenants who edit their config at
//! runtime are honoured on the next escalation — that invariant is
//! covered by the unit tests on the `resolve_expiry` helper inside
//! `hotl_bridge.rs`; here we pin only the integration surface.
//!
//! Tolerance: the test captures `Instant::now()` immediately before
//! invoking the gate, and the gate captures its own `Instant::now()` a
//! few ns later when minting the ticket. We allow 1 second of slack —
//! the same upper bound documented in the task brief.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult};
use xiaoguai_core::hotl_bridge::SuspendingHotlGate;

/// Always-escalate stub. Mirrors `hotl_default_on.rs::AlwaysEscalate`.
#[derive(Debug)]
struct AlwaysEscalate;

#[async_trait]
impl HotlEnforcer for AlwaysEscalate {
    async fn check(&self, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(HotlVerdict::Escalate("test escalate".into()))
    }
}

/// Tolerance for `expires_at` comparison. Captures small ns drift
/// between when the test captures `Instant::now()` and when the gate
/// captures its own `Instant::now()` immediately before minting the
/// ticket. 1s is the same upper bound documented in the S13-7 brief.
const SLACK: Duration = Duration::from_secs(1);

/// Build a fresh gate with a per-scope expiry map and a 24h default.
fn make_gate(expiry: HashMap<String, Duration>) -> (Arc<DecisionRegistry>, SuspendingHotlGate) {
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let default_expiry = Duration::from_secs(24 * 3600);
    let gate = SuspendingHotlGate::with_expiry(enforcer, registry.clone(), default_expiry, expiry);
    (registry, gate)
}

/// Drive the gate once, asserting the verdict is `Suspend`, and return
/// the captured `Instant::now()` (taken just before the gate call) plus
/// the ticket's `expires_at`. Callers compute the delta against the
/// expected window.
async fn drive_and_extract(
    gate: &SuspendingHotlGate,
    registry: &Arc<DecisionRegistry>,
    scope: &str,
) -> (tokio::time::Instant, tokio::time::Instant) {
    let before = tokio::time::Instant::now();
    let verdict =
        <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check(gate, scope, 1.0)
            .await;
    let ticket = match verdict {
        xiaoguai_agent::HotlGateVerdict::Suspend { ticket, .. } => ticket,
        other => panic!("expected Suspend, got {other:?}"),
    };
    assert_eq!(registry.len(), 1, "must register exactly one waiter");
    let expires_at = ticket.expires_at();
    (before, expires_at)
}

fn assert_window(
    before: tokio::time::Instant,
    expires_at: tokio::time::Instant,
    want: Duration,
    label: &str,
) {
    let want_at = before + want;
    let delta = if expires_at >= want_at {
        expires_at - want_at
    } else {
        want_at - expires_at
    };
    assert!(
        delta <= SLACK,
        "{label}: expires_at should be ~now+{want:?}; delta = {delta:?}"
    );
}

/// scope `mcp.oauth.consent` → class `mcp` → 4h (not the 24h default).
#[tokio::test]
async fn expiry_uses_per_scope_class_when_configured() {
    let mut expiry = HashMap::new();
    expiry.insert("mcp".to_string(), Duration::from_secs(4 * 3600));
    let (registry, gate) = make_gate(expiry);

    let (before, expires_at) = drive_and_extract(&gate, &registry, "mcp.oauth.consent").await;
    assert_window(before, expires_at, Duration::from_secs(4 * 3600), "mcp.*");
}

/// scope `tool_call.execute_python` → class `tool_call` → not present →
/// fall back to default `24h`.
#[tokio::test]
async fn expiry_falls_back_to_default_when_class_missing() {
    let mut expiry = HashMap::new();
    expiry.insert("mcp".to_string(), Duration::from_secs(4 * 3600));
    let (registry, gate) = make_gate(expiry);

    let (before, expires_at) =
        drive_and_extract(&gate, &registry, "tool_call.execute_python").await;
    assert_window(
        before,
        expires_at,
        Duration::from_secs(24 * 3600),
        "tool_call.* (fallback)",
    );
}

/// scope without a `.` (e.g. `weird`) → class is the whole scope → not
/// in the map → fall back to default `24h`. Also proves the lookup does
/// not accidentally collide with an empty-string key.
#[tokio::test]
async fn expiry_falls_back_to_default_on_malformed_scope() {
    let mut expiry = HashMap::new();
    expiry.insert("mcp".to_string(), Duration::from_secs(4 * 3600));
    // Seed an empty-string key to prove the malformed-scope path does
    // NOT fall through to it (the helper uses the full scope as the
    // class when there is no `.`, not the empty string).
    expiry.insert(String::new(), Duration::from_secs(1));
    let (registry, gate) = make_gate(expiry);

    let (before, expires_at) = drive_and_extract(&gate, &registry, "weird").await;
    assert_window(
        before,
        expires_at,
        Duration::from_secs(24 * 3600),
        "malformed scope (fallback)",
    );
}
