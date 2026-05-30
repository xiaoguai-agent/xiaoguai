//! HOTL decision-record types + store trait + in-memory implementation.
//!
//! v1.8.x sprint-11 (S11-3a.1): records human verdicts (`allow` / `deny`)
//! against escalated HOTL requests. The agent loop does NOT suspend on
//! `Escalate` in this milestone — `HotlDecisionResponse.resumed` is
//! therefore always `false`. Full suspend/resume is sprint-12+.
//!
//! Production wiring: `xiaoguai-core` provides a `PgHotlDecisionStore` that
//! writes `hotl_decisions` (migration `0026_hotl_decisions.sql`).

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── wire types ────────────────────────────────────────────────────────────────

/// `verdict` field of a HOTL decision.
///
/// Serialised as lowercase (`"allow"` / `"deny"`) on the wire to match the
/// SQL `CHECK` constraint and the chat-ui banner contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotlDecisionVerdict {
    Allow,
    Deny,
}

impl HotlDecisionVerdict {
    /// Wire string for SQL `verdict` column.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// One persisted row in `hotl_decisions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotlDecisionRecord {
    pub id: Uuid,
    pub request_id: Uuid,
    pub tenant_id: Uuid,
    pub verdict: HotlDecisionVerdict,
    pub decided_by: String,
    pub raised_policy_id: Option<Uuid>,
    pub recorded_at: DateTime<Utc>,
}

// ── store error ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum HotlDecisionStoreError {
    /// Unique constraint on `request_id` violated — caller already recorded
    /// a decision for this escalation. Maps to HTTP 409.
    #[error("duplicate request_id: {0}")]
    Duplicate(Uuid),
    /// Generic backend / IO / driver error. Maps to HTTP 500.
    #[error("backend: {0}")]
    Other(String),
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Persistence interface for human HOTL decisions.
///
/// Separated from `HotlPolicyStore` because the lifecycle differs: policies
/// are reusable budgets created at admin-pane time; decisions are one-shot
/// audit records written by `POST /v1/hotl/decisions`.
#[async_trait]
pub trait HotlDecisionStore: Send + Sync + std::fmt::Debug {
    /// Record a decision.
    ///
    /// Returns `Duplicate(request_id)` if a row already exists for the same
    /// `request_id` (idempotency guard via the UNIQUE constraint).
    async fn record(
        &self,
        request_id: Uuid,
        tenant_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: String,
        raised_policy_id: Option<Uuid>,
    ) -> Result<HotlDecisionRecord, HotlDecisionStoreError>;
}

// ── in-memory implementation (tests / dev) ────────────────────────────────────

/// Thread-safe in-memory store used by integration tests and the dev server.
#[derive(Debug, Default)]
pub struct InMemoryHotlDecisionStore {
    inner: parking_lot::Mutex<Vec<HotlDecisionRecord>>,
}

impl InMemoryHotlDecisionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only snapshot of all recorded decisions. Used by tests that
    /// need to assert side effects of `POST /v1/hotl/decisions`.
    #[must_use]
    pub fn snapshot(&self) -> Vec<HotlDecisionRecord> {
        self.inner.lock().clone()
    }
}

#[async_trait]
impl HotlDecisionStore for InMemoryHotlDecisionStore {
    async fn record(
        &self,
        request_id: Uuid,
        tenant_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: String,
        raised_policy_id: Option<Uuid>,
    ) -> Result<HotlDecisionRecord, HotlDecisionStoreError> {
        let mut guard = self.inner.lock();
        if guard.iter().any(|r| r.request_id == request_id) {
            return Err(HotlDecisionStoreError::Duplicate(request_id));
        }
        let record = HotlDecisionRecord {
            id: Uuid::new_v4(),
            request_id,
            tenant_id,
            verdict,
            decided_by,
            raised_policy_id,
            recorded_at: Utc::now(),
        };
        guard.push(record.clone());
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_round_trip() {
        let store = InMemoryHotlDecisionStore::new();
        let request_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();
        let rec = store
            .record(
                request_id,
                tenant_id,
                HotlDecisionVerdict::Allow,
                "alice".into(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(rec.request_id, request_id);
        assert_eq!(rec.tenant_id, tenant_id);
        assert_eq!(rec.verdict, HotlDecisionVerdict::Allow);
        assert_eq!(rec.decided_by, "alice");
        assert!(rec.raised_policy_id.is_none());
    }

    #[tokio::test]
    async fn duplicate_request_id_rejected() {
        let store = InMemoryHotlDecisionStore::new();
        let request_id = Uuid::new_v4();
        store
            .record(
                request_id,
                Uuid::new_v4(),
                HotlDecisionVerdict::Allow,
                "a".into(),
                None,
            )
            .await
            .unwrap();
        let err = store
            .record(
                request_id,
                Uuid::new_v4(),
                HotlDecisionVerdict::Deny,
                "b".into(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, HotlDecisionStoreError::Duplicate(id) if id == request_id));
    }

    #[tokio::test]
    async fn raised_policy_id_round_trips() {
        let store = InMemoryHotlDecisionStore::new();
        let policy_id = Uuid::new_v4();
        let rec = store
            .record(
                Uuid::new_v4(),
                Uuid::new_v4(),
                HotlDecisionVerdict::Allow,
                "alice".into(),
                Some(policy_id),
            )
            .await
            .unwrap();
        assert_eq!(rec.raised_policy_id, Some(policy_id));
    }

    #[test]
    fn verdict_serialises_lowercase() {
        let s = serde_json::to_string(&HotlDecisionVerdict::Allow).unwrap();
        assert_eq!(s, "\"allow\"");
        let s = serde_json::to_string(&HotlDecisionVerdict::Deny).unwrap();
        assert_eq!(s, "\"deny\"");
    }

    #[test]
    fn verdict_deserialises_lowercase() {
        let v: HotlDecisionVerdict = serde_json::from_str("\"allow\"").unwrap();
        assert_eq!(v, HotlDecisionVerdict::Allow);
    }
}
