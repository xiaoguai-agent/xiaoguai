//! HOTL policy CRUD — types + store trait + in-memory implementation.
//!
//! Production wiring: `xiaoguai-core` will provide a `PgHotlPolicyStore`
//! that reads/writes `hotl_policies` and appends to `hotl_usage_log` via
//! a single Postgres connection pool.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── wire types ────────────────────────────────────────────────────────────────

/// One row in `hotl_policies`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotlPolicy {
    pub id: Uuid,
    pub tenant_id: Uuid,
    /// Action category this policy applies to (e.g. `"llm_call"`).
    pub scope: String,
    /// Rolling window width in seconds.
    pub window_seconds: i32,
    /// Maximum invocation count within the window. `None` = no count limit.
    pub max_count: Option<i32>,
    /// Maximum cumulative USD cost within the window. `None` = no cost limit.
    pub max_usd: Option<f64>,
    /// Escalation destination (IM channel or email). `None` = deny on breach.
    pub escalate_to: Option<String>,
}

/// Body accepted by `POST /v1/hotl/policies`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateHotlPolicyRequest {
    pub tenant_id: Uuid,
    pub scope: String,
    pub window_seconds: i32,
    pub max_count: Option<i32>,
    pub max_usd: Option<f64>,
    pub escalate_to: Option<String>,
}

// ── store error ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum HotlPolicyStoreError {
    #[error("policy not found: {0}")]
    NotFound(Uuid),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("backend: {0}")]
    Backend(String),
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Storage operations for HOTL policies.
///
/// Separated from the enforcer so the admin REST layer can do CRUD without
/// coupling to the budget-checking logic, and so PG / in-memory
/// implementations stay interchangeable in tests.
#[async_trait]
pub trait HotlPolicyStore: Send + Sync + std::fmt::Debug {
    /// Return all policies for `tenant_id` (optionally filtered by `scope`).
    async fn list(
        &self,
        tenant_id: Uuid,
        scope: Option<&str>,
    ) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError>;

    /// Persist a new policy. Generates and returns the `id`.
    async fn create(
        &self,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError>;

    /// Remove a policy by `id`. Returns `NotFound` if the row is absent.
    async fn delete(&self, id: Uuid) -> Result<(), HotlPolicyStoreError>;

    /// Return all active policies for `(tenant_id, scope)` — called by the
    /// enforcer before every gated action.
    async fn policies_for(
        &self,
        tenant_id: Uuid,
        scope: &str,
    ) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError>;
}

// ── in-memory implementation (tests / dev) ────────────────────────────────────

/// Thread-safe in-memory store for unit / integration tests.
///
/// Uses a `parking_lot::Mutex` so `create`/`delete` are synchronous under the
/// hood (no async locking needed for a vec of a few dozen rows).
#[derive(Debug, Default)]
pub struct InMemoryHotlPolicyStore {
    inner: parking_lot::Mutex<Vec<HotlPolicy>>,
}

impl InMemoryHotlPolicyStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-load a policy (useful in tests that need a specific id).
    pub fn seed(&self, policy: HotlPolicy) {
        self.inner.lock().push(policy);
    }
}

#[async_trait]
impl HotlPolicyStore for InMemoryHotlPolicyStore {
    async fn list(
        &self,
        tenant_id: Uuid,
        scope: Option<&str>,
    ) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError> {
        let guard = self.inner.lock();
        let rows = guard
            .iter()
            .filter(|p| p.tenant_id == tenant_id && scope.is_none_or(|s| p.scope == s))
            .cloned()
            .collect();
        Ok(rows)
    }

    async fn create(
        &self,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError> {
        if req.window_seconds <= 0 {
            return Err(HotlPolicyStoreError::InvalidArgument(
                "window_seconds must be > 0".into(),
            ));
        }
        if req.max_count.is_none() && req.max_usd.is_none() {
            return Err(HotlPolicyStoreError::InvalidArgument(
                "at least one of max_count or max_usd must be set".into(),
            ));
        }
        if let Some(c) = req.max_count {
            if c <= 0 {
                return Err(HotlPolicyStoreError::InvalidArgument(
                    "max_count must be > 0".into(),
                ));
            }
        }
        if let Some(usd) = req.max_usd {
            if usd < 0.0 {
                return Err(HotlPolicyStoreError::InvalidArgument(
                    "max_usd must be >= 0".into(),
                ));
            }
        }

        let policy = HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            scope: req.scope,
            window_seconds: req.window_seconds,
            max_count: req.max_count,
            max_usd: req.max_usd,
            escalate_to: req.escalate_to,
        };
        self.inner.lock().push(policy.clone());
        Ok(policy)
    }

    async fn delete(&self, id: Uuid) -> Result<(), HotlPolicyStoreError> {
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|p| p.id != id);
        if guard.len() == before {
            return Err(HotlPolicyStoreError::NotFound(id));
        }
        Ok(())
    }

    async fn policies_for(
        &self,
        tenant_id: Uuid,
        scope: &str,
    ) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError> {
        self.list(tenant_id, Some(scope)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(scope: &str) -> CreateHotlPolicyRequest {
        CreateHotlPolicyRequest {
            tenant_id: Uuid::new_v4(),
            scope: scope.into(),
            window_seconds: 60,
            max_count: Some(10),
            max_usd: None,
            escalate_to: Some("ops@example.com".into()),
        }
    }

    // ── validation ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rejects_zero_window() {
        let store = InMemoryHotlPolicyStore::new();
        let mut req = make_req("llm_call");
        req.window_seconds = 0;
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_no_limits() {
        let store = InMemoryHotlPolicyStore::new();
        let mut req = make_req("llm_call");
        req.max_count = None;
        req.max_usd = None;
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_zero_count() {
        let store = InMemoryHotlPolicyStore::new();
        let mut req = make_req("llm_call");
        req.max_count = Some(0);
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn rejects_negative_usd() {
        let store = InMemoryHotlPolicyStore::new();
        let req = CreateHotlPolicyRequest {
            tenant_id: Uuid::new_v4(),
            scope: "llm_call".into(),
            window_seconds: 60,
            max_count: None,
            max_usd: Some(-1.0),
            escalate_to: None,
        };
        let err = store.create(req).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::InvalidArgument(_)));
    }

    // ── round-trip ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_then_list_round_trip() {
        let store = InMemoryHotlPolicyStore::new();
        let req = make_req("llm_call");
        let tenant_id = req.tenant_id;
        let created = store.create(req).await.unwrap();
        assert_eq!(created.scope, "llm_call");
        assert_eq!(created.max_count, Some(10));

        let list = store.list(tenant_id, None).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, created.id);
    }

    #[tokio::test]
    async fn list_filters_by_scope() {
        let store = InMemoryHotlPolicyStore::new();
        let tid = Uuid::new_v4();

        store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "llm_call".into(),
                window_seconds: 60,
                max_count: Some(5),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();
        store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "email_send".into(),
                window_seconds: 60,
                max_count: Some(3),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();

        let llm_only = store.list(tid, Some("llm_call")).await.unwrap();
        assert_eq!(llm_only.len(), 1);
        assert_eq!(llm_only[0].scope, "llm_call");
    }

    #[tokio::test]
    async fn delete_existing_succeeds() {
        let store = InMemoryHotlPolicyStore::new();
        let req = make_req("llm_call");
        let tid = req.tenant_id;
        let p = store.create(req).await.unwrap();
        store.delete(p.id).await.unwrap();
        let remaining = store.list(tid, None).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn delete_missing_returns_not_found() {
        let store = InMemoryHotlPolicyStore::new();
        let err = store.delete(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn policies_for_matches_exact_scope() {
        let store = InMemoryHotlPolicyStore::new();
        let tid = Uuid::new_v4();
        store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "llm_call".into(),
                window_seconds: 60,
                max_count: Some(10),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();
        let found = store.policies_for(tid, "llm_call").await.unwrap();
        assert_eq!(found.len(), 1);
        let empty = store.policies_for(tid, "email_send").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn list_does_not_leak_across_tenants() {
        let store = InMemoryHotlPolicyStore::new();
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();
        store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid_a,
                scope: "llm_call".into(),
                window_seconds: 60,
                max_count: Some(5),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();
        let b_rows = store.list(tid_b, None).await.unwrap();
        assert!(
            b_rows.is_empty(),
            "tenant B must not see tenant A's policies"
        );
    }
}
