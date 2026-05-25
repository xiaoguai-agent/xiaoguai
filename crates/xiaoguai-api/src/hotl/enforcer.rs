//! HOTL budget enforcer — window-bucketed counter + cost accumulator.
//!
//! ## Algorithm
//!
//! For each `(tenant_id, scope)` pair the enforcer:
//!
//! 1. Looks up active policies from [`HotlPolicyStore::policies_for`].
//! 2. Sums `amount` entries in `hotl_usage_log` where
//!    `occurred_at >= now() - window_seconds`.
//! 3. Compares the running total against `max_count` / `max_usd`.
//! 4. If any limit is exceeded → [`HotlVerdict::Escalate`] (when
//!    `escalate_to` is set) or [`HotlVerdict::Deny`].
//! 5. On PG error → **fail-closed** (returns [`HotlVerdict::Deny`]).
//!
//! ## Concurrency
//!
//! The in-memory enforcer uses an `Arc<Mutex<_>>` log so N parallel tokio
//! tasks see a consistent counter (the tokio test `concurrent_calls_atomic`
//! validates this). The PG enforcer relies on `SUM()` over the indexed
//! `occurred_at` column — no extra locking needed because each INSERT is
//! isolated and the subsequent SELECT reads committed rows.
//!
//! ## Wired action sites (this milestone)
//!
//! | Site | Status |
//! |---|---|
//! | LLM call (`xiaoguai-runtime::chat_stream`) | wired (reference impl) |
//! | Email send | follow-up (see docs/plans/hotl-followups.md) |
//! | Webhook invoke | follow-up |

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::hotl::policy::{HotlPolicy, HotlPolicyStore, HotlPolicyStoreError};

// ── verdict ───────────────────────────────────────────────────────────────────

/// Decision returned by [`HotlEnforcer::check`].
#[derive(Debug, Clone, PartialEq)]
pub enum HotlVerdict {
    /// Budget is within limits — proceed.
    Allow,
    /// Budget breached; the `reason` string describes which limit was hit and
    /// where to escalate. The caller should log the escalation and allow the
    /// action to continue (human reviews asynchronously).
    Escalate(String),
    /// Budget breached with no `escalate_to`, or the policy store is
    /// unreachable (fail-closed). The caller must abort the action.
    Deny(String),
}

/// Convenience alias returned from [`HotlEnforcer::check`].
pub type HotlVerdictResult = Result<HotlVerdict, HotlEnforcerError>;

#[derive(Debug, thiserror::Error)]
pub enum HotlEnforcerError {
    #[error("policy store: {0}")]
    PolicyStore(#[from] HotlPolicyStoreError),
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Check whether an action is within budget and record it in the usage log.
///
/// * `tenant_id` — the acting tenant.
/// * `scope`     — action category (`"llm_call"`, `"email_send"`, …).
/// * `amount`    — count increment (use `1.0` for invocation counting; pass
///                 the USD cost for cost-budget scopes).
///
/// The enforcer records the event **before** returning the verdict so that
/// concurrent callers see a consistent tally (i.e. recording happens
/// optimistically; on `Deny` the caller must not proceed regardless).
#[async_trait]
pub trait HotlEnforcer: Send + Sync {
    async fn check(&self, tenant_id: Uuid, scope: &str, amount: f64) -> HotlVerdictResult;
}

// ── in-memory implementation (tests) ─────────────────────────────────────────

/// Log entry stored by [`InMemoryHotlEnforcer`].
#[derive(Debug)]
struct LogEntry {
    tenant_id: Uuid,
    scope: String,
    amount: f64,
    recorded_at: Instant,
}

/// Thread-safe in-memory enforcer for unit tests.
///
/// Uses a `parking_lot::Mutex<Vec<LogEntry>>` — no async lock needed because
/// the critical section is microseconds (push + filter-sum).
#[derive(Debug)]
pub struct InMemoryHotlEnforcer {
    store: Arc<dyn HotlPolicyStore>,
    log: Arc<Mutex<Vec<LogEntry>>>,
    /// When `true`, every `policies_for` call returns an error to test
    /// the fail-closed behaviour.
    pub fail_store: bool,
}

impl InMemoryHotlEnforcer {
    #[must_use]
    pub fn new(store: Arc<dyn HotlPolicyStore>) -> Self {
        Self {
            store,
            log: Arc::new(Mutex::new(Vec::new())),
            fail_store: false,
        }
    }

    /// Compute the sum of `amount` in the log for `(tenant_id, scope)`
    /// within the last `window_seconds` seconds. Called with the log
    /// already pre-appended (optimistic insert).
    fn window_sum(&self, tenant_id: Uuid, scope: &str, window: Duration) -> (f64, usize) {
        let cutoff = Instant::now().checked_sub(window).unwrap_or(Instant::now());
        let guard = self.log.lock();
        let mut sum = 0.0_f64;
        let mut count = 0usize;
        for entry in guard.iter() {
            if entry.tenant_id == tenant_id && entry.scope == scope && entry.recorded_at >= cutoff {
                sum += entry.amount;
                count += 1;
            }
        }
        (sum, count)
    }
}

#[async_trait]
impl HotlEnforcer for InMemoryHotlEnforcer {
    async fn check(&self, tenant_id: Uuid, scope: &str, amount: f64) -> HotlVerdictResult {
        // Fail-closed: if the policy store is simulated as broken, deny.
        if self.fail_store {
            return Ok(HotlVerdict::Deny(
                "policy store unavailable (fail-closed)".into(),
            ));
        }

        let policies = match self.store.policies_for(tenant_id, scope).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(?e, "HOTL policy store error — fail-closed");
                return Ok(HotlVerdict::Deny(format!(
                    "policy store error: {e} (fail-closed)"
                )));
            }
        };

        // If no policy is declared for this scope, allow unconditionally.
        if policies.is_empty() {
            return Ok(HotlVerdict::Allow);
        }

        // Optimistic insert before comparison so concurrent callers see it.
        {
            let mut guard = self.log.lock();
            guard.push(LogEntry {
                tenant_id,
                scope: scope.to_owned(),
                amount,
                recorded_at: Instant::now(),
            });
        }

        // Check every policy for this (tenant, scope). If any is breached,
        // return Escalate / Deny based on the first breached policy's
        // `escalate_to`. We pick the strictest outcome (Deny > Escalate).
        let mut verdict = HotlVerdict::Allow;

        for policy in &policies {
            let window = Duration::from_secs(u64::try_from(policy.window_seconds).unwrap_or(0));
            let (sum, count) = self.window_sum(tenant_id, scope, window);

            let count_breached = policy
                .max_count
                .is_some_and(|max| count > usize::try_from(max).unwrap_or(0));
            let usd_breached = policy.max_usd.is_some_and(|max| sum > max);

            if count_breached || usd_breached {
                let reason = build_reason(policy, count, sum);
                let candidate = match &policy.escalate_to {
                    Some(dest) => HotlVerdict::Escalate(format!("{reason} → escalate to {dest}")),
                    None => HotlVerdict::Deny(reason),
                };
                // Deny beats Escalate.
                verdict = match (&verdict, &candidate) {
                    (HotlVerdict::Allow, _) | (HotlVerdict::Escalate(_), HotlVerdict::Deny(_)) => {
                        candidate
                    }
                    _ => verdict,
                };
            }
        }

        Ok(verdict)
    }
}

fn build_reason(policy: &HotlPolicy, count: usize, sum: f64) -> String {
    let mut parts = Vec::new();
    if let Some(max) = policy.max_count {
        parts.push(format!("count {count} > max_count {max}"));
    }
    if let Some(max) = policy.max_usd {
        parts.push(format!("cost ${sum:.4} > max_usd ${max:.4}"));
    }
    format!(
        "HOTL breach on scope='{}' tenant='{}': {}",
        policy.scope,
        policy.tenant_id,
        parts.join("; ")
    )
}

// ── static stub (route tests) ─────────────────────────────────────────────────

/// Canned enforcer for route-layer tests that don't care about budget logic.
#[derive(Debug, Clone)]
pub struct StaticHotlEnforcer {
    pub verdict: HotlVerdict,
}

impl StaticHotlEnforcer {
    #[must_use]
    pub fn allow() -> Self {
        Self {
            verdict: HotlVerdict::Allow,
        }
    }

    #[must_use]
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            verdict: HotlVerdict::Deny(reason.into()),
        }
    }
}

#[async_trait]
impl HotlEnforcer for StaticHotlEnforcer {
    async fn check(&self, _tenant_id: Uuid, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(self.verdict.clone())
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use super::*;
    use crate::hotl::policy::{CreateHotlPolicyRequest, InMemoryHotlPolicyStore};

    fn store_with_count_policy(
        tenant_id: Uuid,
        scope: &str,
        window_secs: i32,
        max_count: i32,
        escalate_to: Option<&str>,
    ) -> Arc<InMemoryHotlPolicyStore> {
        let store = Arc::new(InMemoryHotlPolicyStore::new());
        let store_clone = Arc::clone(&store);
        // We can't use `await` in a sync helper, so seed directly.
        let policy = crate::hotl::policy::HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id,
            scope: scope.to_owned(),
            window_seconds: window_secs,
            max_count: Some(max_count),
            max_usd: None,
            escalate_to: escalate_to.map(str::to_owned),
        };
        store_clone.seed(policy);
        store
    }

    fn store_with_usd_policy(
        tenant_id: Uuid,
        scope: &str,
        window_secs: i32,
        max_usd: f64,
        escalate_to: Option<&str>,
    ) -> Arc<InMemoryHotlPolicyStore> {
        let store = Arc::new(InMemoryHotlPolicyStore::new());
        let policy = crate::hotl::policy::HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id,
            scope: scope.to_owned(),
            window_seconds: window_secs,
            max_count: None,
            max_usd: Some(max_usd),
            escalate_to: escalate_to.map(str::to_owned),
        };
        store.seed(policy);
        store
    }

    // ── count budget ─────────────────────────────────────────────────────────

    /// 3 calls in a 60s window under a limit of 3 → all Allow.
    #[tokio::test]
    async fn count_under_limit_allows() {
        let tid = Uuid::new_v4();
        let store = store_with_count_policy(tid, "llm_call", 60, 3, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        for _ in 0..3 {
            let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
            assert_eq!(v, HotlVerdict::Allow, "calls 1-3 must be Allow");
        }
    }

    /// 4th call with limit=3 and escalate_to set → Escalate.
    #[tokio::test]
    async fn fourth_call_escalates_when_escalate_to_set() {
        let tid = Uuid::new_v4();
        let store = store_with_count_policy(tid, "llm_call", 60, 3, Some("ops@example.com"));
        let enforcer = InMemoryHotlEnforcer::new(store);

        for _ in 0..3 {
            let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
            assert_eq!(v, HotlVerdict::Allow);
        }
        let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert!(
            matches!(v, HotlVerdict::Escalate(_)),
            "4th call must Escalate, got {v:?}"
        );
        if let HotlVerdict::Escalate(reason) = v {
            assert!(
                reason.contains("ops@example.com"),
                "reason must contain escalation dest: {reason}"
            );
        }
    }

    /// 4th call with limit=3 and no escalate_to → Deny.
    #[tokio::test]
    async fn fourth_call_denies_without_escalate_to() {
        let tid = Uuid::new_v4();
        let store = store_with_count_policy(tid, "llm_call", 60, 3, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        for _ in 0..3 {
            enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        }
        let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert!(
            matches!(v, HotlVerdict::Deny(_)),
            "4th call must Deny, got {v:?}"
        );
    }

    // ── cost (USD) budget ────────────────────────────────────────────────────

    /// Cumulative cost within window stays under max_usd → Allow.
    #[tokio::test]
    async fn cost_under_limit_allows() {
        let tid = Uuid::new_v4();
        let store = store_with_usd_policy(tid, "llm_call", 60, 1.0, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        // Three calls at $0.30 each = $0.90 < $1.00.
        for _ in 0..3 {
            let v = enforcer.check(tid, "llm_call", 0.30).await.unwrap();
            assert_eq!(v, HotlVerdict::Allow);
        }
    }

    /// Cumulative cost exceeds max_usd → Deny (no escalate_to).
    #[tokio::test]
    async fn cost_over_limit_denies() {
        let tid = Uuid::new_v4();
        let store = store_with_usd_policy(tid, "llm_call", 60, 1.0, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        // Two calls at $0.60 each = $1.20 > $1.00.
        enforcer.check(tid, "llm_call", 0.60).await.unwrap();
        let v = enforcer.check(tid, "llm_call", 0.60).await.unwrap();
        assert!(
            matches!(v, HotlVerdict::Deny(_)),
            "cost breach must Deny, got {v:?}"
        );
    }

    // ── no policy → unconditional allow ─────────────────────────────────────

    #[tokio::test]
    async fn no_policy_allows_unconditionally() {
        let store = Arc::new(InMemoryHotlPolicyStore::new());
        let enforcer = InMemoryHotlEnforcer::new(store);
        let v = enforcer
            .check(Uuid::new_v4(), "llm_call", 1.0)
            .await
            .unwrap();
        assert_eq!(v, HotlVerdict::Allow);
    }

    // ── fail-closed ──────────────────────────────────────────────────────────

    /// When the policy store is down, check must return Deny (fail-closed).
    #[tokio::test]
    async fn fail_closed_on_store_error() {
        let store = Arc::new(InMemoryHotlPolicyStore::new());
        let mut enforcer = InMemoryHotlEnforcer::new(store);
        enforcer.fail_store = true;

        let v = enforcer
            .check(Uuid::new_v4(), "llm_call", 1.0)
            .await
            .unwrap();
        assert!(
            matches!(v, HotlVerdict::Deny(_)),
            "fail-closed must return Deny, got {v:?}"
        );
    }

    // ── scope isolation ──────────────────────────────────────────────────────

    /// A policy on 'llm_call' must not affect 'email_send' counts.
    #[tokio::test]
    async fn scope_isolated() {
        let tid = Uuid::new_v4();
        let store = store_with_count_policy(tid, "llm_call", 60, 3, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        for _ in 0..10 {
            let v = enforcer.check(tid, "email_send", 1.0).await.unwrap();
            assert_eq!(
                v,
                HotlVerdict::Allow,
                "email_send has no policy → always Allow"
            );
        }
    }

    // ── concurrent calls — atomic counter correctness ────────────────────────

    /// Spawn N tasks that each call `check` once. The count recorded in the
    /// log must equal N and be consistent (no double-counts, no drops).
    #[tokio::test]
    async fn concurrent_calls_atomic() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        const N: usize = 50;
        let tid = Uuid::new_v4();
        // Limit is higher than N so all calls are allowed — we just want to
        // verify the counter is exact.
        let store = store_with_count_policy(tid, "llm_call", 60, i32::try_from(N * 2).expect("N*2 fits i32"), None);
        let enforcer = Arc::new(InMemoryHotlEnforcer::new(store));
        let allow_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let e = Arc::clone(&enforcer);
            let ac = Arc::clone(&allow_count);
            handles.push(tokio::spawn(async move {
                let v = e.check(tid, "llm_call", 1.0).await.unwrap();
                if v == HotlVerdict::Allow {
                    ac.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(
            allow_count.load(Ordering::Relaxed),
            N,
            "all {N} concurrent calls must be Allow (limit {limit})",
            limit = N * 2
        );
        // The log must contain exactly N entries.
        let log_len = enforcer.log.lock().len();
        assert_eq!(log_len, N, "log must have exactly {N} entries");
    }

    // ── static enforcer (stub) ───────────────────────────────────────────────

    #[tokio::test]
    async fn static_enforcer_allow() {
        let e = StaticHotlEnforcer::allow();
        let v = e.check(Uuid::new_v4(), "llm_call", 1.0).await.unwrap();
        assert_eq!(v, HotlVerdict::Allow);
    }

    #[tokio::test]
    async fn static_enforcer_deny() {
        let e = StaticHotlEnforcer::deny("budget exceeded");
        let v = e.check(Uuid::new_v4(), "llm_call", 1.0).await.unwrap();
        assert!(matches!(v, HotlVerdict::Deny(ref r) if r == "budget exceeded"));
    }

    // ── deny beats escalate when two policies conflict ────────────────────────

    #[tokio::test]
    async fn deny_beats_escalate_for_same_scope() {
        let tid = Uuid::new_v4();
        let store = Arc::new(InMemoryHotlPolicyStore::new());

        // Policy 1: escalate on breach (has escalate_to).
        store.seed(crate::hotl::policy::HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id: tid,
            scope: "llm_call".to_owned(),
            window_seconds: 60,
            max_count: Some(1),
            max_usd: None,
            escalate_to: Some("ops@example.com".to_owned()),
        });
        // Policy 2: deny on breach (no escalate_to), stricter limit.
        store.seed(crate::hotl::policy::HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id: tid,
            scope: "llm_call".to_owned(),
            window_seconds: 60,
            max_count: Some(1),
            max_usd: None,
            escalate_to: None,
        });

        let enforcer = InMemoryHotlEnforcer::new(store);
        enforcer.check(tid, "llm_call", 1.0).await.unwrap(); // 1st call
        let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap(); // 2nd → breach
        assert!(
            matches!(v, HotlVerdict::Deny(_)),
            "Deny must win over Escalate when both policies breach: {v:?}"
        );
    }

    // ── PG bucket query correctness (in-memory proxy) ─────────────────────────
    //
    // This test validates the window-sum arithmetic that the PG query would
    // replicate with `SUM(amount) WHERE occurred_at >= now() - interval`.
    // The in-memory enforcer uses `Instant` durations; we verify that entries
    // appended well before the window are ignored by the sum.

    #[tokio::test]
    async fn old_entries_outside_window_not_counted() {
        let tid = Uuid::new_v4();
        // Very short window: 1 second.
        let store = store_with_count_policy(tid, "llm_call", 1, 3, None);
        let enforcer = InMemoryHotlEnforcer::new(store);

        // Inject 3 "old" log entries directly (bypassing check) with a
        // recorded_at that is 2s in the past — outside the 1s window.
        {
            let old_time = Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or(Instant::now());
            let mut guard = enforcer.log.lock();
            for _ in 0..3 {
                guard.push(LogEntry {
                    tenant_id: tid,
                    scope: "llm_call".to_owned(),
                    amount: 1.0,
                    recorded_at: old_time,
                });
            }
        }

        // Now 3 fresh calls: they should all be within budget because the
        // old entries fall outside the window.
        for i in 0..3 {
            let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
            assert_eq!(
                v,
                HotlVerdict::Allow,
                "fresh call {i} must be Allow (old entries outside window)"
            );
        }
    }

    // ── CRUD round-trip (policy store + enforcer together) ────────────────────

    #[tokio::test]
    async fn crud_then_enforce_round_trip() {
        let store = Arc::new(InMemoryHotlPolicyStore::new());
        let tid = Uuid::new_v4();

        // Create via store.
        let policy = store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "llm_call".into(),
                window_seconds: 60,
                max_count: Some(2),
                max_usd: None,
                escalate_to: Some("ops@example.com".into()),
            })
            .await
            .unwrap();

        let store_trait: Arc<dyn HotlPolicyStore> = Arc::clone(&store) as _;
        let enforcer = InMemoryHotlEnforcer::new(store_trait);

        // 2 calls → Allow.
        for _ in 0..2 {
            let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
            assert_eq!(v, HotlVerdict::Allow);
        }
        // 3rd call → Escalate.
        let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert!(matches!(v, HotlVerdict::Escalate(_)));

        // Delete the policy.
        store.delete(policy.id).await.unwrap();

        // Now any call must be Allow again (no policy).
        let v = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert_eq!(
            v,
            HotlVerdict::Allow,
            "after policy deletion, all calls must be Allow"
        );
    }
}
