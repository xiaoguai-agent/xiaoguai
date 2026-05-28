//! v1.2.3 — PG-backed `HotlPolicyStore` + `HotlEnforcer`.
//!
//! `PgHotlPolicyStore` — CRUD on `hotl_policies` (migration 0011).
//! `PgHotlEnforcer`   — inserts into `hotl_usage_log` then compares windowed
//! SUMs against the active policies. Fail-closed: any PG error → Deny.
//!
//! Lives in `xiaoguai-core` (same layering pattern as `audit_bridge.rs`):
//! the api crate stays sqlx-free; SQL lives here.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use xiaoguai_api::hotl::{
    enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult},
    policy::{CreateHotlPolicyRequest, HotlPolicy, HotlPolicyStore, HotlPolicyStoreError},
};

// ── policy store ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PgHotlPolicyStore {
    pool: PgPool,
}

impl PgHotlPolicyStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn pg_err(e: sqlx::Error) -> HotlPolicyStoreError {
    HotlPolicyStoreError::Backend(e.to_string())
}

/// Map a raw DB row to `HotlPolicy`.
///
/// The schema uses `NUMERIC(10,4)` for `max_usd`. sqlx decodes NUMERIC
/// as `bigdecimal::BigDecimal` (when the feature is on) or as `String`
/// depending on the sqlx feature set; we bind via the `query` (dynamic)
/// API and extract as `Option<f64>` via `try_get`. For `max_count` the
/// column is INT so `i32` maps directly.
#[derive(sqlx::FromRow)]
struct PolicyRow {
    id: Uuid,
    tenant_id: Uuid,
    scope: String,
    window_seconds: i32,
    max_count: Option<i32>,
    max_usd: Option<f64>,
    escalate_to: Option<String>,
}

impl From<PolicyRow> for HotlPolicy {
    fn from(r: PolicyRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            scope: r.scope,
            window_seconds: r.window_seconds,
            max_count: r.max_count,
            max_usd: r.max_usd,
            escalate_to: r.escalate_to,
        }
    }
}

#[async_trait]
impl HotlPolicyStore for PgHotlPolicyStore {
    async fn list(
        &self,
        tenant_id: Uuid,
        scope: Option<&str>,
    ) -> Result<Vec<HotlPolicy>, HotlPolicyStoreError> {
        // Use a dynamic query to handle the optional `scope` filter cleanly.
        let rows: Vec<PolicyRow> = if let Some(s) = scope {
            sqlx::query_as(
                "SELECT id, tenant_id, scope, window_seconds, \
                        max_count, max_usd::FLOAT8, escalate_to \
                 FROM hotl_policies \
                 WHERE tenant_id = $1 AND scope = $2 \
                 ORDER BY created_at ASC",
            )
            .bind(tenant_id)
            .bind(s)
            .fetch_all(&self.pool)
            .await
            .map_err(pg_err)?
        } else {
            sqlx::query_as(
                "SELECT id, tenant_id, scope, window_seconds, \
                        max_count, max_usd::FLOAT8, escalate_to \
                 FROM hotl_policies \
                 WHERE tenant_id = $1 \
                 ORDER BY created_at ASC",
            )
            .bind(tenant_id)
            .fetch_all(&self.pool)
            .await
            .map_err(pg_err)?
        };
        Ok(rows.into_iter().map(HotlPolicy::from).collect())
    }

    async fn create(
        &self,
        req: CreateHotlPolicyRequest,
    ) -> Result<HotlPolicy, HotlPolicyStoreError> {
        // Mirror the validation in `InMemoryHotlPolicyStore` so the PG
        // implementation is consistent.
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

        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO hotl_policies \
                (id, tenant_id, scope, window_seconds, max_count, max_usd, escalate_to) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(req.tenant_id)
        .bind(&req.scope)
        .bind(req.window_seconds)
        .bind(req.max_count)
        .bind(req.max_usd)
        .bind(&req.escalate_to)
        .execute(&self.pool)
        .await
        .map_err(pg_err)?;

        Ok(HotlPolicy {
            id,
            tenant_id: req.tenant_id,
            scope: req.scope,
            window_seconds: req.window_seconds,
            max_count: req.max_count,
            max_usd: req.max_usd,
            escalate_to: req.escalate_to,
        })
    }

    async fn delete(&self, id: Uuid) -> Result<(), HotlPolicyStoreError> {
        let result = sqlx::query("DELETE FROM hotl_policies WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(pg_err)?;
        if result.rows_affected() == 0 {
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

// ── enforcer ──────────────────────────────────────────────────────────────────

/// PG-backed enforcer.
///
/// Algorithm (mirrors the in-memory enforcer doc):
/// 1. Look up active policies via `policies_for`.
/// 2. INSERT into `hotl_usage_log` (optimistic, before comparison).
/// 3. SUM `amount` WHERE `occurred_at >= now() - INTERVAL '? seconds'`.
/// 4. Compare against `max_count` / `max_usd`.
/// 5. PG error → fail-closed (Deny).
#[derive(Debug, Clone)]
pub struct PgHotlEnforcer {
    pool: PgPool,
    store: Arc<PgHotlPolicyStore>,
}

impl PgHotlEnforcer {
    #[must_use]
    pub fn new(pool: PgPool, store: Arc<PgHotlPolicyStore>) -> Self {
        Self { pool, store }
    }
}

#[async_trait]
impl HotlEnforcer for PgHotlEnforcer {
    async fn check(&self, tenant_id: Uuid, scope: &str, amount: f64) -> HotlVerdictResult {
        // Fetch active policies; fail-closed on error.
        let policies = match self.store.policies_for(tenant_id, scope).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(?e, "HOTL policy store error — fail-closed");
                return Ok(HotlVerdict::Deny(format!(
                    "policy store error: {e} (fail-closed)"
                )));
            }
        };

        // No policy declared → unconditional allow.
        if policies.is_empty() {
            return Ok(HotlVerdict::Allow);
        }

        // Optimistic insert before comparison (same semantics as in-memory).
        if let Err(e) =
            sqlx::query("INSERT INTO hotl_usage_log (tenant_id, scope, amount) VALUES ($1, $2, $3)")
                .bind(tenant_id)
                .bind(scope)
                .bind(amount)
                .execute(&self.pool)
                .await
        {
            tracing::error!(?e, "HOTL usage log insert failed — fail-closed");
            return Ok(HotlVerdict::Deny(format!(
                "usage log insert error: {e} (fail-closed)"
            )));
        }

        let mut verdict = HotlVerdict::Allow;

        for policy in &policies {
            // Windowed SUM: count and cost aggregated in one query.
            // Use `$3 * interval '1 second'` to safely bind an integer
            // window_seconds without relying on string interpolation.
            let row: (Option<f64>, Option<f64>) = match sqlx::query_as(
                "SELECT COUNT(*)::FLOAT8, SUM(amount)::FLOAT8 \
                 FROM hotl_usage_log \
                 WHERE tenant_id = $1 \
                   AND scope = $2 \
                   AND occurred_at >= now() - ($3 * interval '1 second')",
            )
            .bind(tenant_id)
            .bind(scope)
            .bind(i64::from(policy.window_seconds))
            .fetch_one(&self.pool)
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(?e, "HOTL window SUM failed — fail-closed");
                    return Ok(HotlVerdict::Deny(format!(
                        "window sum error: {e} (fail-closed)"
                    )));
                }
            };

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let count = row.0.unwrap_or(0.0) as usize;
            let sum = row.1.unwrap_or(0.0);

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
                // Deny beats Escalate (same rule as in-memory enforcer).
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

// ── HotlGate adapter (Tier-2 prereq) ─────────────────────────────────────────
//
// `xiaoguai-agent::HotlGate` is the abstract trait the ReAct loop consults
// before each tool dispatch. It deliberately lives in `xiaoguai-agent` (not
// `xiaoguai-api`) to avoid the `api → agent → api` dep cycle. `EnforcerGate`
// bridges the full `HotlEnforcer` (api crate) into the minimal `HotlGate`
// surface the loop needs.
//
// Mapping rules:
//   * `Allow`               → `HotlGateVerdict::Allow`
//   * `Escalate(reason)`    → `HotlGateVerdict::Allow` + `tracing::warn`
//                             (the policy author explicitly chose async human
//                             review over blocking; the loop must proceed)
//   * `Deny(reason)`        → `HotlGateVerdict::Deny(reason)`
//   * Enforcer infra error  → `HotlGateVerdict::Deny("…")` + `tracing::error`
//                             (fail-closed — matches the upstream
//                              `send_message` contract)

/// Adapter that lets the full `HotlEnforcer` plug into the agent's
/// `HotlGate` slot. Construct in `run_serve` once, share via `Arc`.
///
/// `HotlEnforcer` does not require `Debug`, so we implement it manually
/// (with an opaque payload) to satisfy the `HotlGate: Debug` super-bound.
#[derive(Clone)]
pub struct EnforcerGate {
    inner: Arc<dyn HotlEnforcer>,
}

impl std::fmt::Debug for EnforcerGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnforcerGate")
            .field("inner", &"Arc<dyn HotlEnforcer>")
            .finish()
    }
}

impl EnforcerGate {
    #[must_use]
    pub fn new(inner: Arc<dyn HotlEnforcer>) -> Self {
        Self { inner }
    }

    /// Box-and-Arc helper so callers don't have to repeat the dyn coercion.
    #[must_use]
    pub fn arc(inner: Arc<dyn HotlEnforcer>) -> Arc<dyn xiaoguai_agent::HotlGate> {
        Arc::new(Self::new(inner))
    }
}

#[async_trait]
impl xiaoguai_agent::HotlGate for EnforcerGate {
    async fn check(
        &self,
        tenant_id: Uuid,
        scope: &str,
        amount: f64,
    ) -> xiaoguai_agent::HotlGateVerdict {
        match self.inner.check(tenant_id, scope, amount).await {
            Ok(HotlVerdict::Allow) => xiaoguai_agent::HotlGateVerdict::Allow,
            Ok(HotlVerdict::Escalate(reason)) => {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    %scope,
                    %reason,
                    "HOTL gate escalation — proceeding with tool dispatch"
                );
                xiaoguai_agent::HotlGateVerdict::Allow
            }
            Ok(HotlVerdict::Deny(reason)) => {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    %scope,
                    %reason,
                    "HOTL gate denied tool dispatch"
                );
                xiaoguai_agent::HotlGateVerdict::Deny(reason)
            }
            Err(e) => {
                // Fail-closed: enforcer-itself errored. Distinct from
                // "no enforcer wired" (Option<None>), which never reaches
                // this adapter.
                tracing::error!(
                    tenant_id = %tenant_id,
                    %scope,
                    error = %e,
                    "HOTL gate enforcer error — fail-closed deny"
                );
                xiaoguai_agent::HotlGateVerdict::Deny(format!(
                    "HOTL enforcer infrastructure error (fail-closed): {e}"
                ))
            }
        }
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── unit tests (pure logic, no DB) ────────────────────────────────────────

    #[test]
    fn build_reason_formats_count_and_usd() {
        let policy = HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            scope: "llm_call".into(),
            window_seconds: 60,
            max_count: Some(5),
            max_usd: Some(1.5),
            escalate_to: None,
        };
        let reason = build_reason(&policy, 6, 2.0);
        assert!(reason.contains("count 6 > max_count 5"));
        assert!(reason.contains("cost $2.0000 > max_usd $1.5000"));
    }

    #[test]
    fn build_reason_count_only() {
        let policy = HotlPolicy {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            scope: "email_send".into(),
            window_seconds: 3600,
            max_count: Some(10),
            max_usd: None,
            escalate_to: None,
        };
        let reason = build_reason(&policy, 11, 0.0);
        assert!(reason.contains("count 11 > max_count 10"));
        assert!(!reason.contains("max_usd"));
    }

    // ── PG integration tests ──────────────────────────────────────────────────
    // Run with: DATABASE_URL=postgres://... cargo test -p xiaoguai-core
    //           --ignore-rust-version -- --ignored hotl_pg_

    async fn pg_pool() -> sqlx::PgPool {
        let url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for PG bridge tests");
        sqlx::PgPool::connect(&url).await.expect("pg connect")
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn hotl_pg_store_create_list_delete() {
        let pool = pg_pool().await;
        let store = PgHotlPolicyStore::new(pool);
        let tid = Uuid::new_v4();

        let created = store
            .create(xiaoguai_api::hotl::policy::CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "llm_call".into(),
                window_seconds: 3600,
                max_count: Some(100),
                max_usd: None,
                escalate_to: Some("ops@example.com".into()),
            })
            .await
            .unwrap();

        let list = store.list(tid, None).await.unwrap();
        assert!(list.iter().any(|p| p.id == created.id));

        // Scoped filter works.
        let scoped = store.list(tid, Some("llm_call")).await.unwrap();
        assert_eq!(scoped.len(), 1);
        let empty = store.list(tid, Some("email_send")).await.unwrap();
        assert!(empty.is_empty());

        store.delete(created.id).await.unwrap();
        let after = store.list(tid, None).await.unwrap();
        assert!(after.iter().all(|p| p.id != created.id));
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn hotl_pg_store_delete_missing_is_not_found() {
        let pool = pg_pool().await;
        let store = PgHotlPolicyStore::new(pool);
        let err = store.delete(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(
            err,
            xiaoguai_api::hotl::policy::HotlPolicyStoreError::NotFound(_)
        ));
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn hotl_pg_store_tenant_isolation() {
        let pool = pg_pool().await;
        let store = PgHotlPolicyStore::new(pool);
        let tid_a = Uuid::new_v4();
        let tid_b = Uuid::new_v4();

        store
            .create(xiaoguai_api::hotl::policy::CreateHotlPolicyRequest {
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
        assert!(b_rows.is_empty(), "tenant B must not see tenant A rows");
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn hotl_pg_enforcer_no_policy_allows() {
        let pool = pg_pool().await;
        let store = Arc::new(PgHotlPolicyStore::new(pool.clone()));
        let enforcer = PgHotlEnforcer::new(pool, store);
        let v = enforcer
            .check(Uuid::new_v4(), "llm_call", 1.0)
            .await
            .unwrap();
        assert_eq!(v, HotlVerdict::Allow);
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn hotl_pg_enforcer_count_breach_denies() {
        let pool = pg_pool().await;
        let store = Arc::new(PgHotlPolicyStore::new(pool.clone()));
        let tid = Uuid::new_v4();

        store
            .create(xiaoguai_api::hotl::policy::CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: "llm_call".into(),
                window_seconds: 3600,
                max_count: Some(2),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();

        let enforcer = PgHotlEnforcer::new(pool, store);
        let v1 = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        let v2 = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert_eq!(v1, HotlVerdict::Allow);
        assert_eq!(v2, HotlVerdict::Allow);
        let v3 = enforcer.check(tid, "llm_call", 1.0).await.unwrap();
        assert!(
            matches!(v3, HotlVerdict::Deny(_)),
            "3rd call must Deny: {v3:?}"
        );
    }
}
