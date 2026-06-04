//! Conflict detection and arbitration for in-flight agent runs.
//!
//! # Model
//!
//! A **resource** is the pair `(resource_kind, resource_id)` — e.g.
//! `("invoice", "INV-001")`.  Only one agent may *write* to a resource at
//! a time; concurrent reads are unconstrained (callers decide what constitutes a
//! write by choosing when to acquire a run lock).
//!
//! When a second dispatch is attempted on the same resource while a run is in
//! flight, the `ConflictArbitrator` enforces one of three policies:
//!
//! | Policy | Behaviour |
//! |--------|-----------|
//! | `Reject` | Return `Err(AgentConflict)` immediately. |
//! | `Queue`  | Async-wait until the in-flight run releases, then proceed. |
//! | `CancelOther` | Mark the in-flight run cancelled; proceed with the new one. |
//!
//! # Implementation
//!
//! In-flight state is tracked with an `Arc<Mutex<RunSlot>>` per resource key.
//! A `RunGuard` is returned to callers; dropping the guard releases the slot.
//!
//! # Deferred
//! - Queue depth limit (currently unbounded; a bounded semaphore is v1.3).
//! - Cross-process resource locking via a PG advisory lock or Redis SETNX.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::{oneshot, Notify};

// ── Resource key ─────────────────────────────────────────────────────────────

/// The identity of a resource that an agent run operates on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub resource_kind: String,
    pub resource_id: String,
}

impl ResourceKey {
    pub fn new(resource_kind: impl Into<String>, resource_id: impl Into<String>) -> Self {
        Self {
            resource_kind: resource_kind.into(),
            resource_id: resource_id.into(),
        }
    }
}

impl std::fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.resource_kind, self.resource_id)
    }
}

// ── ConflictPolicy ────────────────────────────────────────────────────────────

/// How to arbitrate when two agent runs target the same resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Reject the incoming run immediately.
    Reject,
    /// Block the incoming run until the in-flight run completes.
    Queue,
    /// Cancel the in-flight run and proceed with the incoming one.
    CancelOther,
}

// ── Errors ────────────────────────────────────────────────────────────────────

/// Returned when a dispatch is rejected due to a resource conflict.
#[derive(Debug)]
pub struct AgentConflict {
    pub resource: ResourceKey,
    pub in_flight_agent: String,
    pub policy: ConflictPolicy,
}

impl std::fmt::Display for AgentConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AgentConflict on resource '{}': agent '{}' already in flight (policy: {:?})",
            self.resource, self.in_flight_agent, self.policy
        )
    }
}

// ── Internal state ────────────────────────────────────────────────────────────

/// Shared state for a single resource slot.
#[derive(Debug)]
struct RunSlot {
    /// Name of the agent currently holding the slot.
    agent_name: String,
    /// When the run started.
    started_at: Instant,
    /// Waiters queued under `Queue` policy; each gets notified on release.
    waiters: Vec<Arc<Notify>>,
    /// Under `CancelOther` policy, this sender fires to the in-flight run.
    cancel_tx: Option<oneshot::Sender<()>>,
    /// Whether the slot has been marked cancelled (by `CancelOther`).
    cancelled: bool,
}

/// A live lock on a resource slot.  Dropping this releases the slot and
/// notifies any waiters.
#[derive(Debug)]
pub struct RunGuard {
    key: ResourceKey,
    table: Arc<Mutex<HashMap<ResourceKey, Arc<Mutex<RunSlot>>>>>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut table = self.table.lock().unwrap();
        if let Some(slot_arc) = table.remove(&self.key) {
            let mut slot = slot_arc.lock().unwrap();
            // Wake all waiters.
            for notify in slot.waiters.drain(..) {
                notify.notify_one();
            }
        }
    }
}

// ── ConflictArbitrator ────────────────────────────────────────────────────────

/// Tracks in-flight agent runs per resource and enforces conflict policies.
#[derive(Clone)]
pub struct ConflictArbitrator {
    table: Arc<Mutex<HashMap<ResourceKey, Arc<Mutex<RunSlot>>>>>,
    policy: ConflictPolicy,
}

impl ConflictArbitrator {
    /// Create an arbitrator with a specific policy applied to all resources.
    #[must_use]
    pub fn new(policy: ConflictPolicy) -> Self {
        Self {
            table: Arc::new(Mutex::new(HashMap::new())),
            policy,
        }
    }

    /// Attempt to acquire the lock for `resource` on behalf of `agent_name`.
    ///
    /// - `Reject`: returns `Err(AgentConflict)` if the resource is busy.
    /// - `Queue`: returns `Ok(guard)` after the current holder releases.
    /// - `CancelOther`: signals the current holder's cancel channel, then
    ///   acquires the slot.
    ///
    /// Returns a `RunGuard`; dropping it releases the slot.
    ///
    /// # Errors
    /// Returns `AgentConflict` when the policy is `Reject` and the resource is
    /// already held by another agent.
    ///
    /// # Panics
    /// Panics if any internal mutex is poisoned.
    pub async fn acquire(
        &self,
        resource: ResourceKey,
        agent_name: impl Into<String>,
    ) -> Result<RunGuard, AgentConflict> {
        let agent_name = agent_name.into();

        loop {
            // Check current state under lock.
            let (action, notify_arc) = {
                let mut table = self.table.lock().unwrap();

                if let Some(slot_arc) = table.get(&resource) {
                    let mut slot = slot_arc.lock().unwrap();
                    match self.policy {
                        ConflictPolicy::Reject => {
                            return Err(AgentConflict {
                                resource,
                                in_flight_agent: slot.agent_name.clone(),
                                policy: self.policy,
                            });
                        }
                        ConflictPolicy::Queue => {
                            // Register a waiter.
                            let notify = Arc::new(Notify::new());
                            slot.waiters.push(Arc::clone(&notify));
                            (QueueAction::Wait, Some(notify))
                        }
                        ConflictPolicy::CancelOther => {
                            // Fire the cancel channel if present.
                            if let Some(tx) = slot.cancel_tx.take() {
                                let _ = tx.send(());
                            }
                            slot.cancelled = true;
                            // Evict the slot so we can insert our own below.
                            drop(slot);
                            table.remove(&resource);
                            (QueueAction::Insert, None)
                        }
                    }
                } else {
                    (QueueAction::Insert, None)
                }
            };

            match action {
                QueueAction::Wait => {
                    // Wait outside the mutex.
                    if let Some(notify) = notify_arc {
                        notify.notified().await;
                    }
                    // Loop back to try again.
                }
                QueueAction::Insert => {
                    // Slot is free — insert our entry.
                    let (cancel_tx, _cancel_rx) = oneshot::channel::<()>();
                    let slot = Arc::new(Mutex::new(RunSlot {
                        agent_name,
                        started_at: Instant::now(),
                        waiters: Vec::new(),
                        cancel_tx: Some(cancel_tx),
                        cancelled: false,
                    }));
                    {
                        let mut table = self.table.lock().unwrap();
                        table.insert(resource.clone(), slot);
                    }
                    return Ok(RunGuard {
                        key: resource,
                        table: Arc::clone(&self.table),
                    });
                }
            }
        }
    }

    /// Return the agent name currently holding `resource`, if any.
    ///
    /// # Panics
    /// Panics if any internal mutex is poisoned.
    #[must_use]
    pub fn current_holder(&self, resource: &ResourceKey) -> Option<String> {
        let table = self.table.lock().unwrap();
        table.get(resource).map(|slot_arc| {
            let slot = slot_arc.lock().unwrap();
            slot.agent_name.clone()
        })
    }

    /// Return the number of resources currently locked.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn in_flight_count(&self) -> usize {
        self.table.lock().unwrap().len()
    }

    /// Return how long (seconds) `resource` has been held, if it is locked.
    ///
    /// # Panics
    /// Panics if any internal mutex is poisoned.
    #[must_use]
    pub fn hold_duration_secs(&self, resource: &ResourceKey) -> Option<f64> {
        let table = self.table.lock().unwrap();
        table.get(resource).map(|slot_arc| {
            let slot = slot_arc.lock().unwrap();
            slot.started_at.elapsed().as_secs_f64()
        })
    }
}

// Tiny private enum to communicate the action back outside the lock.
enum QueueAction {
    Wait,
    Insert,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OrchestratorError;
    use tokio::time::{timeout, Duration};

    fn res(id: &str) -> ResourceKey {
        ResourceKey::new("invoice", id)
    }

    // ── Reject policy ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reject_policy_first_acquire_succeeds() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        let _guard = arb.acquire(res("INV-001"), "agent-A").await.unwrap();
        assert_eq!(arb.in_flight_count(), 1);
    }

    #[tokio::test]
    async fn reject_policy_second_acquire_fails_with_conflict() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        let _guard = arb.acquire(res("INV-001"), "agent-A").await.unwrap();
        let result = arb.acquire(res("INV-001"), "agent-B").await;
        assert!(result.is_err());
        let conflict = result.unwrap_err();
        assert_eq!(conflict.in_flight_agent, "agent-A");
        assert_eq!(conflict.policy, ConflictPolicy::Reject);
    }

    #[tokio::test]
    async fn reject_policy_different_resource_no_conflict() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        let _guard_a = arb.acquire(res("INV-001"), "agent-A").await.unwrap();
        // Different resource id — should succeed.
        let _guard_b = arb.acquire(res("INV-002"), "agent-B").await.unwrap();
        assert_eq!(arb.in_flight_count(), 2);
    }

    #[tokio::test]
    async fn reject_policy_guard_drop_releases_slot() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        {
            let _guard = arb.acquire(res("INV-001"), "agent-A").await.unwrap();
            assert_eq!(arb.in_flight_count(), 1);
        }
        // After drop the slot is gone.
        assert_eq!(arb.in_flight_count(), 0);
        // A new acquire on the same resource must succeed.
        let _guard2 = arb.acquire(res("INV-001"), "agent-B").await.unwrap();
        assert_eq!(arb.in_flight_count(), 1);
    }

    // ── Queue policy ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_policy_second_acquire_blocks_until_release() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Queue);
        let arb2 = arb.clone();

        let guard_a = arb.acquire(res("INV-100"), "agent-A").await.unwrap();

        // Spawn B which will block until A's guard is dropped.
        let handle =
            tokio::spawn(async move { arb2.acquire(res("INV-100"), "agent-B").await.unwrap() });

        // Give the spawn a brief moment to register.
        tokio::task::yield_now().await;

        // B should still be blocked; drop A's guard.
        drop(guard_a);

        // B must acquire within a short window.
        let _guard_b = timeout(Duration::from_millis(200), handle)
            .await
            .expect("timed out waiting for queue to unblock")
            .expect("task panicked");
    }

    // ── CancelOther policy ────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_other_policy_evicts_existing_holder() {
        let arb = ConflictArbitrator::new(ConflictPolicy::CancelOther);
        let _guard_a = arb.acquire(res("INV-200"), "agent-A").await.unwrap();
        assert_eq!(arb.current_holder(&res("INV-200")).unwrap(), "agent-A");

        // B cancels A and takes the slot.
        let _guard_b = arb.acquire(res("INV-200"), "agent-B").await.unwrap();
        assert_eq!(arb.current_holder(&res("INV-200")).unwrap(), "agent-B");
        assert_eq!(arb.in_flight_count(), 1);
    }

    // ── current_holder / in_flight_count ─────────────────────────────────────

    #[tokio::test]
    async fn current_holder_returns_none_when_free() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        assert!(arb.current_holder(&res("INV-999")).is_none());
    }

    #[tokio::test]
    async fn in_flight_count_reflects_active_guards() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        let g1 = arb.acquire(res("R1"), "a1").await.unwrap();
        let g2 = arb.acquire(res("R2"), "a2").await.unwrap();
        assert_eq!(arb.in_flight_count(), 2);
        drop(g1);
        assert_eq!(arb.in_flight_count(), 1);
        drop(g2);
        assert_eq!(arb.in_flight_count(), 0);
    }

    // ── OrchestratorError conversion ──────────────────────────────────────────

    #[tokio::test]
    async fn conflict_error_can_become_orchestrator_error() {
        let arb = ConflictArbitrator::new(ConflictPolicy::Reject);
        let _g = arb.acquire(res("INV-001"), "agent-A").await.unwrap();
        let result: Result<RunGuard, OrchestratorError> = arb
            .acquire(res("INV-001"), "agent-B")
            .await
            .map_err(|c| OrchestratorError::Internal(c.to_string()));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("agent-A"));
    }
}
