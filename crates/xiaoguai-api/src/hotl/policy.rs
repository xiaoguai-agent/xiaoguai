//! HOTL policy CRUD — types + store trait + in-memory implementation.
//!
//! Production wiring: `xiaoguai-core` will provide a `SqliteHotlPolicyStore`
//! that reads/writes `hotl_policies` and appends to `hotl_usage_log` via
//! a single `SQLite` connection pool.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── wire types ────────────────────────────────────────────────────────────────

/// One row in `hotl_policies`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotlPolicy {
    pub id: Uuid,
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

/// Shared validation for `create`/`update` request bodies — keeps the
/// in-memory and SQLite stores enforcing identical invariants.
///
/// # Errors
/// Returns `InvalidArgument` when `window_seconds <= 0`, neither limit is set,
/// `max_count <= 0`, or `max_usd < 0`.
pub fn validate_policy_request(
    req: &CreateHotlPolicyRequest,
) -> Result<(), HotlPolicyStoreError> {
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
    Ok(())
}

// ── trait ─────────────────────────────────────────────────────────────────────

/// Storage operations for HOTL policies.
///
/// Separated from the enforcer so the admin REST layer can do CRUD without
/// coupling to the budget-checking logic, and so PG / in-memory
/// implementations stay interchangeable in tests.
#[async_trait]
pub trait HotlPolicyStore: Send + Sync + std::fmt::Debug {
    /// Return all policies (optionally filtered by `scope`).
    async fn list(&self, scope: Option<&str>) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError>;

    /// Persist a new policy. Generates and returns the `id`.
    async fn create(
        &self,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError>;

    /// Replace the mutable fields of policy `id` from `req` (same shape +
    /// validation as `create`). Returns `NotFound` if the row is absent.
    async fn update(
        &self,
        id: Uuid,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError>;

    /// Remove a policy by `id`. Returns `NotFound` if the row is absent.
    async fn delete(&self, id: Uuid) -> Result<(), HotlPolicyStoreError>;

    /// Return all active policies for `scope` — called by the enforcer
    /// before every gated action.
    async fn policies_for(&self, scope: &str) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError>;
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
    async fn list(&self, scope: Option<&str>) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError> {
        let guard = self.inner.lock();
        let rows = guard
            .iter()
            .filter(|p| scope.is_none_or(|s| p.scope == s))
            .cloned()
            .collect();
        Ok(rows)
    }

    async fn create(
        &self,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError> {
        validate_policy_request(&req)?;

        let policy = HotlPolicy {
            id: Uuid::new_v4(),
            scope: req.scope,
            window_seconds: req.window_seconds,
            max_count: req.max_count,
            max_usd: req.max_usd,
            escalate_to: req.escalate_to,
        };
        self.inner.lock().push(policy.clone());
        Ok(policy)
    }

    async fn update(
        &self,
        id: Uuid,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError> {
        validate_policy_request(&req)?;
        let mut guard = self.inner.lock();
        let policy = guard
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(HotlPolicyStoreError::NotFound(id))?;
        policy.scope = req.scope;
        policy.window_seconds = req.window_seconds;
        policy.max_count = req.max_count;
        policy.max_usd = req.max_usd;
        policy.escalate_to = req.escalate_to;
        Ok(policy.clone())
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

    async fn policies_for(&self, scope: &str) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError> {
        self.list(Some(scope)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(scope: &str) -> CreateHotlPolicyRequest {
        CreateHotlPolicyRequest {
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
        let created = store.create(req).await.unwrap();
        assert_eq!(created.scope, "llm_call");
        assert_eq!(created.max_count, Some(10));

        let list = store.list(None).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, created.id);
    }

    #[tokio::test]
    async fn list_filters_by_scope() {
        let store = InMemoryHotlPolicyStore::new();

        store
            .create(CreateHotlPolicyRequest {
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
                scope: "email_send".into(),
                window_seconds: 60,
                max_count: Some(3),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();

        let llm_only = store.list(Some("llm_call")).await.unwrap();
        assert_eq!(llm_only.len(), 1);
        assert_eq!(llm_only[0].scope, "llm_call");
    }

    #[tokio::test]
    async fn delete_existing_succeeds() {
        let store = InMemoryHotlPolicyStore::new();
        let req = make_req("llm_call");
        let p = store.create(req).await.unwrap();
        store.delete(p.id).await.unwrap();
        let remaining = store.list(None).await.unwrap();
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
        store
            .create(CreateHotlPolicyRequest {
                scope: "llm_call".into(),
                window_seconds: 60,
                max_count: Some(10),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();
        let found = store.policies_for("llm_call").await.unwrap();
        assert_eq!(found.len(), 1);
        let empty = store.policies_for("email_send").await.unwrap();
        assert!(empty.is_empty());
    }

    // ── update ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn update_existing_replaces_fields() {
        let store = InMemoryHotlPolicyStore::new();
        let created = store.create(make_req("llm_call")).await.unwrap();
        let updated = store
            .update(
                created.id,
                CreateHotlPolicyRequest {
                    scope: "email_send".into(),
                    window_seconds: 120,
                    max_count: Some(3),
                    max_usd: Some(1.5),
                    escalate_to: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.id, created.id, "id is preserved across update");
        assert_eq!(updated.scope, "email_send");
        assert_eq!(updated.window_seconds, 120);
        assert_eq!(updated.max_usd, Some(1.5));
        let list = store.list(None).await.unwrap();
        assert_eq!(list.len(), 1, "update must not add a row");
        assert_eq!(list[0].scope, "email_send");
    }

    #[tokio::test]
    async fn update_missing_returns_not_found() {
        let store = InMemoryHotlPolicyStore::new();
        let err = store
            .update(Uuid::new_v4(), make_req("llm_call"))
            .await
            .unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_validates_request() {
        let store = InMemoryHotlPolicyStore::new();
        let created = store.create(make_req("llm_call")).await.unwrap();
        let mut bad = make_req("llm_call");
        bad.window_seconds = 0;
        let err = store.update(created.id, bad).await.unwrap_err();
        assert!(matches!(err, HotlPolicyStoreError::InvalidArgument(_)));
    }
}
