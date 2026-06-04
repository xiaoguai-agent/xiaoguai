//! Capability eval suite — `HotL` escalation routing.
//!
//! Covers the seven scenarios specified in the task brief:
//!
//! 1. Under-threshold  — 10 events with threshold=100 → Allow.
//! 2. At threshold     — exactly N events with threshold=N → Allow
//!                       (breach condition is `count > max_count`, exclusive).
//! 3. Above threshold  — N+1 events → Escalate to the configured destination.
//! 4. Window expiry    — events older than the window must not count.
//! 5. Policy hot-update — delete + re-create a policy; next check uses new threshold.
//! 6. Approver tier routing — `escalate_to` string carries the right tier destination.
//!
//! The in-memory store + enforcer are the same fixtures used by the unit tests
//! in `crates/xiaoguai-api/src/hotl/enforcer.rs`.

use std::sync::Arc;

use uuid::Uuid;
use xiaoguai_api::hotl::{
    enforcer::{HotlVerdict, InMemoryHotlEnforcer},
    policy::HotlPolicy,
    HotlEnforcer, HotlPolicyStore, InMemoryHotlPolicyStore,
};

// ── fixture helpers ───────────────────────────────────────────────────────────

/// Build a store pre-seeded with a single count-budget policy.
fn count_store(
    scope: &str,
    window_secs: i32,
    max_count: i32,
    escalate_to: Option<&str>,
) -> Arc<InMemoryHotlPolicyStore> {
    let store = Arc::new(InMemoryHotlPolicyStore::new());
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: scope.to_owned(),
        window_seconds: window_secs,
        max_count: Some(max_count),
        max_usd: None,
        escalate_to: escalate_to.map(str::to_owned),
    });
    store
}

/// Build a store pre-seeded with a single USD-budget policy.
fn usd_store(
    scope: &str,
    window_secs: i32,
    max_usd: f64,
    escalate_to: Option<&str>,
) -> Arc<InMemoryHotlPolicyStore> {
    let store = Arc::new(InMemoryHotlPolicyStore::new());
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: scope.to_owned(),
        window_seconds: window_secs,
        max_count: None,
        max_usd: Some(max_usd),
        escalate_to: escalate_to.map(str::to_owned),
    });
    store
}

// ── scenario 1: under-threshold ───────────────────────────────────────────────

/// 10 usage events with threshold=100 must all return Allow.
#[tokio::test]
async fn eval_under_threshold_all_allow() {
    let store = count_store("llm_call", 3600, 100, Some("ops@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    for i in 0..10 {
        let verdict = enforcer.check("llm_call", 1.0).await.unwrap();
        assert_eq!(
            verdict,
            HotlVerdict::Allow,
            "event {i} of 10 must be Allow (threshold=100)"
        );
    }
}

// ── scenario 2: at threshold ──────────────────────────────────────────────────

/// Exactly N events when threshold=N must return Allow (breach is count > max,
/// so equality is not a breach).
#[tokio::test]
async fn eval_at_threshold_still_allow() {
    const N: i32 = 15;
    let store = count_store("llm_call", 3600, N, Some("ops@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    for i in 0..N {
        let verdict = enforcer.check("llm_call", 1.0).await.unwrap();
        assert_eq!(
            verdict,
            HotlVerdict::Allow,
            "event {i} of {N} must be Allow (at-threshold, breach is count > max)"
        );
    }
}

// ── scenario 3: above threshold → escalate ───────────────────────────────────

/// The (N+1)-th event when threshold=N must Escalate when `escalate_to` is set.
#[tokio::test]
async fn eval_above_threshold_escalates() {
    const N: i32 = 10;
    let dest = "team-alpha@example.com";
    let store = count_store("llm_call", 3600, N, Some(dest));
    let enforcer = InMemoryHotlEnforcer::new(store);

    // N calls consume the budget (all Allow).
    for _ in 0..N {
        enforcer.check("llm_call", 1.0).await.unwrap();
    }

    // (N+1)-th call must Escalate.
    let verdict = enforcer.check("llm_call", 1.0).await.unwrap();
    assert!(
        matches!(verdict, HotlVerdict::Escalate(_)),
        "call {n} must Escalate; got {verdict:?}",
        n = N + 1
    );

    // Escalation reason must contain the destination address.
    if let HotlVerdict::Escalate(reason) = verdict {
        assert!(
            reason.contains(dest),
            "escalation reason must contain destination '{dest}': {reason}"
        );
    }
}

/// The (N+1)-th event when threshold=N with no `escalate_to` must Deny.
#[tokio::test]
async fn eval_above_threshold_denies_when_no_escalate_to() {
    const N: i32 = 5;
    let store = count_store("llm_call", 3600, N, None);
    let enforcer = InMemoryHotlEnforcer::new(store);

    for _ in 0..N {
        enforcer.check("llm_call", 1.0).await.unwrap();
    }

    let verdict = enforcer.check("llm_call", 1.0).await.unwrap();
    assert!(
        matches!(verdict, HotlVerdict::Deny(_)),
        "breach with no escalate_to must Deny; got {verdict:?}"
    );
}

// ── scenario 4: window expiry ─────────────────────────────────────────────────

/// Events from outside the rolling window must not count toward the budget.
///
/// Strategy: use a 1-second window, make N calls to push the counter high,
/// sleep 1.1 s so all entries fall outside the window, then make N fresh calls
/// — they should all be Allow because the old entries are excluded.
#[tokio::test]
async fn eval_window_expiry_old_events_excluded() {
    const N: i32 = 5; // threshold = 5 (at-threshold, still Allow)
    let store = count_store("llm_call", 1, N, Some("ops@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    // Fill the window to threshold.
    for _ in 0..N {
        let v = enforcer.check("llm_call", 1.0).await.unwrap();
        assert_eq!(v, HotlVerdict::Allow, "initial fills must be Allow");
    }

    // Wait until the 1-second window expires.
    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;

    // Fresh calls in the new window: threshold applies from zero again.
    for i in 0..N {
        let v = enforcer.check("llm_call", 1.0).await.unwrap();
        assert_eq!(
            v,
            HotlVerdict::Allow,
            "post-expiry call {i} must be Allow (old entries outside 1s window)"
        );
    }
}

// ── scenario 5: policy hot-update ────────────────────────────────────────────

/// Deleting the old policy and creating a tighter one mid-flight must take
/// effect on the very next `check` call.
#[tokio::test]
async fn eval_policy_hot_update_tighter_threshold() {
    let store = Arc::new(InMemoryHotlPolicyStore::new());

    // Start with a generous budget of 50.
    let policy = {
        let p = HotlPolicy {
            id: Uuid::new_v4(),
            scope: "llm_call".to_owned(),
            window_seconds: 3600,
            max_count: Some(50),
            max_usd: None,
            escalate_to: Some("low-tier@example.com".to_owned()),
        };
        store.seed(p.clone());
        p
    };

    let enforcer = InMemoryHotlEnforcer::new(Arc::clone(&store) as _);

    // 10 calls pass comfortably under the 50-call budget.
    for _ in 0..10 {
        let v = enforcer.check("llm_call", 1.0).await.unwrap();
        assert_eq!(
            v,
            HotlVerdict::Allow,
            "calls 1-10 must be Allow under budget=50"
        );
    }

    // Hot-update: remove generous policy, insert tighter one (threshold=10,
    // which is exactly the count already recorded — so allow is still OK, but
    // an 11th call must escalate).
    store.delete(policy.id).await.unwrap();
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: "llm_call".to_owned(),
        window_seconds: 3600,
        max_count: Some(10), // equal to current count → next call is count=11 > 10
        max_usd: None,
        escalate_to: Some("high-tier@example.com".to_owned()),
    });

    // 11th call must escalate under the new tighter policy.
    let v = enforcer.check("llm_call", 1.0).await.unwrap();
    assert!(
        matches!(v, HotlVerdict::Escalate(_)),
        "11th call must Escalate after policy hot-update (threshold→10); got {v:?}"
    );
    if let HotlVerdict::Escalate(reason) = v {
        assert!(
            reason.contains("high-tier@example.com"),
            "escalation must route to new destination; got: {reason}"
        );
    }
}

/// Relaxing the threshold mid-flight must let previously-blocked calls through.
#[tokio::test]
async fn eval_policy_hot_update_relaxed_threshold() {
    let store = Arc::new(InMemoryHotlPolicyStore::new());

    // Start with a tight budget of 2.
    let policy = {
        let p = HotlPolicy {
            id: Uuid::new_v4(),
            scope: "llm_call".to_owned(),
            window_seconds: 3600,
            max_count: Some(2),
            max_usd: None,
            escalate_to: None,
        };
        store.seed(p.clone());
        p
    };

    let enforcer = InMemoryHotlEnforcer::new(Arc::clone(&store) as _);

    // 2 calls at-threshold → Allow.
    for _ in 0..2 {
        enforcer.check("llm_call", 1.0).await.unwrap();
    }
    // 3rd call → Deny (tight policy).
    let v = enforcer.check("llm_call", 1.0).await.unwrap();
    assert!(
        matches!(v, HotlVerdict::Deny(_)),
        "3rd call must Deny; got {v:?}"
    );

    // Relax: delete old policy, insert generous one (threshold=1000).
    store.delete(policy.id).await.unwrap();
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: "llm_call".to_owned(),
        window_seconds: 3600,
        max_count: Some(1000),
        max_usd: None,
        escalate_to: None,
    });

    // Next call must now be Allow under the relaxed policy.
    let v = enforcer.check("llm_call", 1.0).await.unwrap();
    assert_eq!(
        v,
        HotlVerdict::Allow,
        "call after relaxing threshold to 1000 must be Allow"
    );
}

// ── scenario 7: approver tier routing ────────────────────────────────────────
//
// The HotL policy DSL has a single `escalate_to` string (no native tier enum).
// Operators encode tier by convention:
//   - Low-risk actions:    "tier-1@example.com"  (e.g. email_send)
//   - Medium-risk actions: "tier-2@example.com"  (e.g. llm_call)
//   - High-risk actions:   "tier-3@example.com"  (e.g. external_api)
//
// These tests verify that the enforcer faithfully round-trips the
// `escalate_to` string into the Escalate verdict so downstream routing
// (IM gateway, webhook dispatcher) receives the correct tier address.

/// Low-risk scope (`email_send`) → Escalate carries tier-1 address.
#[tokio::test]
async fn eval_tier_routing_low_risk() {
    let store = count_store("email_send", 3600, 1, Some("tier-1@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    enforcer.check("email_send", 1.0).await.unwrap(); // 1 call at-threshold
    let v = enforcer.check("email_send", 1.0).await.unwrap(); // breach

    assert!(
        matches!(v, HotlVerdict::Escalate(_)),
        "must Escalate for low-risk tier; got {v:?}"
    );
    if let HotlVerdict::Escalate(reason) = v {
        assert!(
            reason.contains("tier-1@example.com"),
            "low-risk escalation must route to tier-1; got: {reason}"
        );
    }
}

/// Medium-risk scope (`llm_call`) → Escalate carries tier-2 address.
#[tokio::test]
async fn eval_tier_routing_medium_risk() {
    let store = count_store("llm_call", 3600, 1, Some("tier-2@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    enforcer.check("llm_call", 1.0).await.unwrap();
    let v = enforcer.check("llm_call", 1.0).await.unwrap();

    assert!(
        matches!(v, HotlVerdict::Escalate(_)),
        "must Escalate for medium-risk tier; got {v:?}"
    );
    if let HotlVerdict::Escalate(reason) = v {
        assert!(
            reason.contains("tier-2@example.com"),
            "medium-risk escalation must route to tier-2; got: {reason}"
        );
    }
}

/// High-risk scope (`external_api`) → Escalate carries tier-3 address.
#[tokio::test]
async fn eval_tier_routing_high_risk() {
    let store = count_store("external_api", 3600, 1, Some("tier-3@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    enforcer.check("external_api", 1.0).await.unwrap();
    let v = enforcer.check("external_api", 1.0).await.unwrap();

    assert!(
        matches!(v, HotlVerdict::Escalate(_)),
        "must Escalate for high-risk tier; got {v:?}"
    );
    if let HotlVerdict::Escalate(reason) = v {
        assert!(
            reason.contains("tier-3@example.com"),
            "high-risk escalation must route to tier-3; got: {reason}"
        );
    }
}

/// Deny takes precedence over Escalate when two policies conflict — the
/// tier-routing layer must never silently downgrade a Deny to an Escalate.
#[tokio::test]
async fn eval_tier_routing_deny_beats_escalate() {
    let store = Arc::new(InMemoryHotlPolicyStore::new());

    // Policy 1: escalate to tier-2 on breach.
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: "llm_call".to_owned(),
        window_seconds: 3600,
        max_count: Some(1),
        max_usd: None,
        escalate_to: Some("tier-2@example.com".to_owned()),
    });
    // Policy 2: hard-deny on breach (no escalate_to).
    store.seed(HotlPolicy {
        id: Uuid::new_v4(),
        scope: "llm_call".to_owned(),
        window_seconds: 3600,
        max_count: Some(1),
        max_usd: None,
        escalate_to: None,
    });

    let enforcer = InMemoryHotlEnforcer::new(store);
    enforcer.check("llm_call", 1.0).await.unwrap(); // 1st call
    let v = enforcer.check("llm_call", 1.0).await.unwrap(); // breach

    assert!(
        matches!(v, HotlVerdict::Deny(_)),
        "Deny must win over tier-2 Escalate when both policies breach; got {v:?}"
    );
}

// ── bonus: USD cost budget escalation ────────────────────────────────────────

/// USD budget breach with `escalate_to` set must Escalate (not Deny).
#[tokio::test]
async fn eval_usd_budget_breach_escalates() {
    // max_usd = $1.00, escalate to ops.
    let store = usd_store("llm_call", 3600, 1.0, Some("billing@example.com"));
    let enforcer = InMemoryHotlEnforcer::new(store);

    // $0.60 call is within budget.
    let v = enforcer.check("llm_call", 0.60).await.unwrap();
    assert_eq!(v, HotlVerdict::Allow, "first $0.60 call must be Allow");

    // Second $0.60 call pushes cumulative to $1.20 > $1.00 → Escalate.
    let v = enforcer.check("llm_call", 0.60).await.unwrap();
    assert!(
        matches!(v, HotlVerdict::Escalate(_)),
        "cumulative $1.20 > $1.00 must Escalate; got {v:?}"
    );
    if let HotlVerdict::Escalate(reason) = v {
        assert!(
            reason.contains("billing@example.com"),
            "USD-breach escalation must carry billing address; got: {reason}"
        );
    }
}

/// No policy for scope → unconditional Allow regardless of amount.
#[tokio::test]
async fn eval_no_policy_unconditional_allow() {
    let store = Arc::new(InMemoryHotlPolicyStore::new());
    let enforcer = InMemoryHotlEnforcer::new(store);

    for i in 0..50 {
        let v = enforcer.check("llm_call", 999.0).await.unwrap();
        assert_eq!(
            v,
            HotlVerdict::Allow,
            "call {i} must Allow when no policy exists"
        );
    }
}
