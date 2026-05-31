//! v1.2.3 — PG-backed `HotlPolicyStore` + `HotlEnforcer`.
//!
//! `PgHotlPolicyStore` — CRUD on `hotl_policies` (migration 0011).
//! `PgHotlEnforcer`   — inserts into `hotl_usage_log` then compares windowed
//! SUMs against the active policies. Fail-closed: any PG error → Deny.
//!
//! Lives in `xiaoguai-core` (same layering pattern as `audit_bridge.rs`):
//! the api crate stays sqlx-free; SQL lives here.
//!
//! Sprint-12 S12-7: adds `PgHotlDecisionStore` (table `hotl_decisions`,
//! migration 0026) and `PgHotlAuditSink` (adapter over
//! `xiaoguai_audit::PgAuditSink`). Together they replace the production
//! `state.hotl_decision_store = None` / `state.hotl_audit = None` slots
//! set by the v1.8.1 hotfix, flipping `POST /v1/hotl/decisions` from 503
//! → 201 in production.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use xiaoguai_api::hotl::{
    audit::HotlAuditSink,
    decision::{
        HotlDecisionRecord, HotlDecisionStore, HotlDecisionStoreError, HotlDecisionVerdict,
    },
    enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult},
    policy::{CreateHotlPolicyRequest, HotlPolicy, HotlPolicyStore, HotlPolicyStoreError},
};
use xiaoguai_audit::chain::sink::PgAuditSink;
use xiaoguai_audit::AuditEntry;

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

// ── SuspendingHotlGate (sprint-12 S12-4) ─────────────────────────────────────
//
// Second `HotlGate` adapter alongside `EnforcerGate`. The only difference is
// how `HotlVerdict::Escalate(_)` is mapped:
//
//   EnforcerGate           → log a warn + return `HotlGateVerdict::Allow`
//                            (v1.8.x semantics — the LLM call proceeds)
//   SuspendingHotlGate     → mint a `request_id`, register a waiter on the
//                            shared `DecisionRegistry`, return
//                            `HotlGateVerdict::Suspend { ticket, .. }`
//                            so the ReAct loop blocks on the operator's
//                            decision (sprint-12 v1.9.0 default).
//
// The `Allow`, `Deny(reason)`, and infra-error (`Err(_)` → fail-closed Deny)
// arms are identical to `EnforcerGate` — those paths are not behaviour gates.
//
// Wiring constraint: the `DecisionRegistry` MUST be constructed exactly once
// in `run_serve` and shared between this gate and `AppState.decision_registry`.
// The route handler (`POST /v1/hotl/decisions`, sprint-12 S12-6) calls
// `state.decision_registry.resolve(...)` to wake the parked loop — if the
// gate held a *different* registry, the resolve would silently no-op and the
// loop would hang until the 24h default expiry fires.

/// Sprint-12 (S12-4). Adapter that suspends the ReAct loop on `Escalate`
/// instead of allowing the call through.
///
/// Construct alongside `EnforcerGate` in `run_serve` and select between the
/// two with `agent.hotl.suspend_on_escalate`. The `default_expiry` is the
/// upper bound the loop will block waiting for an operator decision (the
/// design default is 24h; tests pass shorter durations).
pub struct SuspendingHotlGate {
    inner: Arc<dyn HotlEnforcer>,
    registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
    default_expiry: std::time::Duration,
}

impl std::fmt::Debug for SuspendingHotlGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuspendingHotlGate")
            .field("inner", &"Arc<dyn HotlEnforcer>")
            .field("registry", &"Arc<DecisionRegistry>")
            .field("default_expiry", &self.default_expiry)
            .finish()
    }
}

impl SuspendingHotlGate {
    #[must_use]
    pub fn new(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
    ) -> Self {
        Self {
            inner,
            registry,
            default_expiry,
        }
    }

    /// Box-and-Arc helper mirroring `EnforcerGate::arc`. Lets `run_serve`
    /// pick between adapters with a single-line ternary.
    #[must_use]
    pub fn arc(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
    ) -> Arc<dyn xiaoguai_agent::HotlGate> {
        Arc::new(Self::new(inner, registry, default_expiry))
    }
}

#[async_trait]
impl xiaoguai_agent::HotlGate for SuspendingHotlGate {
    async fn check(
        &self,
        tenant_id: Uuid,
        scope: &str,
        amount: f64,
    ) -> xiaoguai_agent::HotlGateVerdict {
        match self.inner.check(tenant_id, scope, amount).await {
            Ok(HotlVerdict::Allow) => xiaoguai_agent::HotlGateVerdict::Allow,
            Ok(HotlVerdict::Deny(reason)) => {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    %scope,
                    %reason,
                    "HOTL gate denied tool dispatch"
                );
                xiaoguai_agent::HotlGateVerdict::Deny(reason)
            }
            Ok(HotlVerdict::Escalate(reason)) => {
                let request_id = Uuid::new_v4();
                let expires_at = tokio::time::Instant::now() + self.default_expiry;
                let ticket = self.registry.register(request_id, expires_at);
                tracing::info!(
                    tenant_id = %tenant_id,
                    %scope,
                    %reason,
                    %request_id,
                    "HOTL escalate → suspend; awaiting operator decision"
                );
                xiaoguai_agent::HotlGateVerdict::Suspend {
                    request_id,
                    scope: scope.to_string(),
                    ticket,
                }
            }
            Err(e) => {
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

/// Sprint-12 (S12-4). Build the per-request `HotlGate` plugged into
/// `AgentConfig`, selecting `EnforcerGate` or `SuspendingHotlGate` based
/// on the `agent.hotl.suspend_on_escalate` config flag.
///
/// Extracted into a free function so `run_serve` stays a single-line
/// selection and `hotl_gate_selection.rs` can prove the table directly
/// without spinning up a full server. The registry is passed in (not
/// constructed here) so both adapters and `AppState` share one instance —
/// see the wiring constraint at the top of the `SuspendingHotlGate` block.
#[must_use]
pub fn build_hotl_gate(
    suspend_on_escalate: bool,
    enforcer: Arc<dyn HotlEnforcer>,
    registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
    default_expiry: std::time::Duration,
) -> Arc<dyn xiaoguai_agent::HotlGate> {
    if suspend_on_escalate {
        SuspendingHotlGate::arc(enforcer, registry, default_expiry)
    } else {
        EnforcerGate::arc(enforcer)
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

// ── decision store (sprint-12 S12-7) ─────────────────────────────────────────
//
// PG-backed `HotlDecisionStore` for `POST /v1/hotl/decisions`. Reads/writes
// the `hotl_decisions` table from migration 0026. The UNIQUE constraint on
// `request_id` is the idempotency guard — a duplicate insert surfaces as
// `HotlDecisionStoreError::Duplicate(request_id)` so the route returns 409.
//
// `verdict` is stored as the lowercase text the SQL CHECK constraint enforces
// (`'allow' | 'deny'`); on read we map both values back into the
// `HotlDecisionVerdict` enum. Any other value (impossible given the CHECK)
// surfaces as a `Backend` error rather than silently coercing.

#[derive(Debug, Clone)]
pub struct PgHotlDecisionStore {
    pool: PgPool,
}

impl PgHotlDecisionStore {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Box-and-Arc helper so callers in `run_serve` don't repeat the dyn coercion.
    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<dyn HotlDecisionStore> {
        Arc::new(Self::new(pool))
    }
}

fn decision_pg_err(e: sqlx::Error) -> HotlDecisionStoreError {
    HotlDecisionStoreError::Other(e.to_string())
}

/// PostgreSQL `unique_violation` code, per
/// <https://www.postgresql.org/docs/current/errcodes-appendix.html>.
const PG_UNIQUE_VIOLATION: &str = "23505";

#[async_trait]
impl HotlDecisionStore for PgHotlDecisionStore {
    async fn record(
        &self,
        request_id: Uuid,
        tenant_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: String,
        raised_policy_id: Option<Uuid>,
    ) -> Result<HotlDecisionRecord, HotlDecisionStoreError> {
        let id = Uuid::new_v4();

        let row: (Uuid, Uuid, Uuid, String, String, Option<Uuid>, DateTime<Utc>) =
            sqlx::query_as(
                "INSERT INTO hotl_decisions \
                    (id, request_id, tenant_id, verdict, decided_by, raised_policy_id) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 RETURNING id, request_id, tenant_id, verdict, decided_by, raised_policy_id, recorded_at",
            )
            .bind(id)
            .bind(request_id)
            .bind(tenant_id)
            .bind(verdict.as_str())
            .bind(&decided_by)
            .bind(raised_policy_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(db_err) = &e {
                    if db_err.code().as_deref() == Some(PG_UNIQUE_VIOLATION) {
                        return HotlDecisionStoreError::Duplicate(request_id);
                    }
                }
                decision_pg_err(e)
            })?;

        let returned_verdict = match row.3.as_str() {
            "allow" => HotlDecisionVerdict::Allow,
            "deny" => HotlDecisionVerdict::Deny,
            other => {
                return Err(HotlDecisionStoreError::Other(format!(
                    "unexpected verdict text from DB: {other:?}"
                )))
            }
        };

        Ok(HotlDecisionRecord {
            id: row.0,
            request_id: row.1,
            tenant_id: row.2,
            verdict: returned_verdict,
            decided_by: row.4,
            raised_policy_id: row.5,
            recorded_at: row.6,
        })
    }
}

// ── audit sink adapter (sprint-12 S12-7) ─────────────────────────────────────
//
// Wraps `xiaoguai_audit::PgAuditSink` behind the api crate's `HotlAuditSink`
// trait so the `/v1/hotl/decisions` route can record `hotl.decision` audit
// entries through the same HMAC-chained sink the rest of the audit surface
// uses. We keep the trait surface (`Result<(), String>`) opaque per
// `xiaoguai_api::hotl::audit` — `ChainError`'s rich variants are squashed to
// a string so the api crate doesn't pull a `xiaoguai-audit` dep.

#[derive(Clone)]
pub struct PgHotlAuditSink {
    inner: Arc<PgAuditSink>,
}

impl PgHotlAuditSink {
    #[must_use]
    pub fn new(inner: Arc<PgAuditSink>) -> Self {
        Self { inner }
    }

    /// Box-and-Arc helper so callers in `run_serve` don't repeat the dyn coercion.
    #[must_use]
    pub fn arc(inner: Arc<PgAuditSink>) -> Arc<dyn HotlAuditSink> {
        Arc::new(Self::new(inner))
    }
}

impl std::fmt::Debug for PgHotlAuditSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgHotlAuditSink")
            .field("inner", &"Arc<PgAuditSink>")
            .finish()
    }
}

#[async_trait]
impl HotlAuditSink for PgHotlAuditSink {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.inner
            .append(entry)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
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

    // ── Sprint-12 S12-4: SuspendingHotlGate adapter tests ────────────────────
    //
    // Behaviour mapping vs. the legacy `EnforcerGate`:
    //
    //   upstream HotlVerdict        EnforcerGate        SuspendingHotlGate
    //   --------------------------  ------------------  ------------------------
    //   Allow                       HGV::Allow          HGV::Allow
    //   Deny(reason)                HGV::Deny(reason)   HGV::Deny(reason)
    //   Escalate(reason)            HGV::Allow + warn   HGV::Suspend{ticket}
    //   Err(_)  (enforcer infra)    HGV::Deny(...)      HGV::Deny(...) (fail-closed)
    //
    // These tests pin the table above and prove the registry is the one
    // construction site (no second registry minted inside the gate).

    /// Tiny inline mock enforcer driven by a stored verdict or error. Avoids
    /// pulling mockall into the dev-deps just for these five tests.
    #[derive(Debug)]
    struct StubEnforcer {
        next: parking_lot::Mutex<Option<HotlVerdictResult>>,
    }

    impl StubEnforcer {
        fn allow() -> Arc<Self> {
            Arc::new(Self {
                next: parking_lot::Mutex::new(Some(Ok(HotlVerdict::Allow))),
            })
        }
        fn deny(reason: &str) -> Arc<Self> {
            Arc::new(Self {
                next: parking_lot::Mutex::new(Some(Ok(HotlVerdict::Deny(reason.into())))),
            })
        }
        fn escalate(reason: &str) -> Arc<Self> {
            Arc::new(Self {
                next: parking_lot::Mutex::new(Some(Ok(HotlVerdict::Escalate(reason.into())))),
            })
        }
        fn infra_error(msg: &str) -> Arc<Self> {
            Arc::new(Self {
                next: parking_lot::Mutex::new(Some(Err(
                    xiaoguai_api::hotl::enforcer::HotlEnforcerError::PolicyStore(
                        xiaoguai_api::hotl::policy::HotlPolicyStoreError::Backend(msg.into()),
                    ),
                ))),
            })
        }
    }

    #[async_trait]
    impl xiaoguai_api::hotl::enforcer::HotlEnforcer for StubEnforcer {
        async fn check(
            &self,
            _tenant_id: Uuid,
            _scope: &str,
            _amount: f64,
        ) -> xiaoguai_api::hotl::enforcer::HotlVerdictResult {
            // Clone the stored verdict for each call so the same stub can be
            // consulted multiple times (the gate-selection integration test
            // wants two .check() calls against the same enforcer).
            let guard = self.next.lock();
            match guard.as_ref().expect("StubEnforcer not primed") {
                Ok(v) => Ok(v.clone()),
                Err(xiaoguai_api::hotl::enforcer::HotlEnforcerError::PolicyStore(
                    xiaoguai_api::hotl::policy::HotlPolicyStoreError::Backend(s),
                )) => Err(
                    xiaoguai_api::hotl::enforcer::HotlEnforcerError::PolicyStore(
                        xiaoguai_api::hotl::policy::HotlPolicyStoreError::Backend(s.clone()),
                    ),
                ),
                Err(_) => unreachable!("StubEnforcer only primes Backend errors"),
            }
        }
    }

    fn registry() -> Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry> {
        xiaoguai_api::hotl::decision_registry::DecisionRegistry::arc()
    }

    fn default_expiry() -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    #[tokio::test]
    async fn suspending_gate_allow_passes_through() {
        let reg = registry();
        let enforcer = StubEnforcer::allow();
        let gate = SuspendingHotlGate::new(enforcer, reg.clone(), default_expiry());

        let v =
            xiaoguai_agent::HotlGate::check(&gate, Uuid::new_v4(), "tool_call.search", 1.0).await;
        assert!(matches!(v, xiaoguai_agent::HotlGateVerdict::Allow));
        assert!(reg.is_empty(), "Allow path must not register a ticket");
    }

    #[tokio::test]
    async fn suspending_gate_deny_passes_through() {
        let reg = registry();
        let enforcer = StubEnforcer::deny("budget exceeded");
        let gate = SuspendingHotlGate::new(enforcer, reg.clone(), default_expiry());

        let v =
            xiaoguai_agent::HotlGate::check(&gate, Uuid::new_v4(), "tool_call.search", 1.0).await;
        match v {
            xiaoguai_agent::HotlGateVerdict::Deny(reason) => {
                assert_eq!(reason, "budget exceeded");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
        assert!(reg.is_empty(), "Deny path must not register a ticket");
    }

    #[tokio::test]
    async fn suspending_gate_escalate_returns_suspend_with_registered_ticket() {
        let reg = registry();
        let enforcer = StubEnforcer::escalate("monthly budget at 110%");
        let gate = SuspendingHotlGate::new(enforcer, reg.clone(), default_expiry());

        let v =
            xiaoguai_agent::HotlGate::check(&gate, Uuid::new_v4(), "tool_call.execute_python", 1.0)
                .await;
        let (request_id, scope, ticket) = match v {
            xiaoguai_agent::HotlGateVerdict::Suspend {
                request_id,
                scope,
                ticket,
            } => (request_id, scope, ticket),
            other => panic!("expected Suspend, got {other:?}"),
        };
        assert_eq!(scope, "tool_call.execute_python");
        assert_eq!(
            reg.len(),
            1,
            "Suspend path must register exactly one waiter"
        );

        // Resolve via the registry and confirm the ticket receives the verdict.
        // Use the agent-crate hotl_gate types directly (the registry's
        // `pub use` makes them reachable via the api path too, but the
        // explicit `xiaoguai_agent::hotl_gate::*` form documents the
        // canonical source).
        let resolved = reg.resolve(
            request_id,
            xiaoguai_agent::hotl_gate::HotlDecisionVerdict {
                verdict: xiaoguai_agent::hotl_gate::HotlResolution::Allow,
                decided_by: Some("alice@example.com".into()),
                recorded_at: chrono::Utc::now(),
            },
        );
        assert!(resolved, "live waiter must be resolved");
        assert!(reg.is_empty(), "resolve must remove the entry");

        let cancel = tokio_util::sync::CancellationToken::new();
        let got = ticket
            .await_decision(&cancel)
            .await
            .expect("ticket must yield the resolved verdict");
        assert_eq!(
            got.verdict,
            xiaoguai_agent::hotl_gate::HotlResolution::Allow
        );
        assert_eq!(got.decided_by.as_deref(), Some("alice@example.com"));
    }

    #[tokio::test]
    async fn suspending_gate_infrastructure_error_fails_closed() {
        let reg = registry();
        let enforcer = StubEnforcer::infra_error("pg connection refused");
        let gate = SuspendingHotlGate::new(enforcer, reg.clone(), default_expiry());

        let v =
            xiaoguai_agent::HotlGate::check(&gate, Uuid::new_v4(), "tool_call.search", 1.0).await;
        match v {
            xiaoguai_agent::HotlGateVerdict::Deny(reason) => {
                assert!(
                    reason.contains("HOTL enforcer infrastructure error"),
                    "expected fail-closed deny reason, got: {reason}"
                );
            }
            other => panic!("expected Deny on infra error, got {other:?}"),
        }
        assert!(
            reg.is_empty(),
            "Err path must not register a ticket (fail-closed before mint)"
        );
    }

    #[tokio::test]
    async fn suspending_gate_uses_passed_registry_not_new_one() {
        // Pin the wiring contract: the gate must register against the
        // registry it was constructed with, not a fresh one. We prove this
        // by minting a waiter externally on the same registry, calling the
        // gate (which must register a *second* waiter), then asserting both
        // waiters coexist independently — meaning they share the same
        // DashMap (a different map would have len==1, not len==2).
        let reg = registry();
        let preexisting_id = Uuid::new_v4();
        let _preexisting_ticket = reg.register(
            preexisting_id,
            tokio::time::Instant::now() + std::time::Duration::from_secs(60),
        );
        assert_eq!(reg.len(), 1);

        let enforcer = StubEnforcer::escalate("budget breach");
        let gate = SuspendingHotlGate::new(enforcer, reg.clone(), default_expiry());

        let v =
            xiaoguai_agent::HotlGate::check(&gate, Uuid::new_v4(), "tool_call.search", 1.0).await;
        let (new_request_id, _scope, _ticket) = match v {
            xiaoguai_agent::HotlGateVerdict::Suspend {
                request_id,
                scope,
                ticket,
            } => (request_id, scope, ticket),
            other => panic!("expected Suspend, got {other:?}"),
        };
        assert_ne!(new_request_id, preexisting_id);
        assert_eq!(
            reg.len(),
            2,
            "shared registry must hold both waiters; got {} (gate likely minted its own registry)",
            reg.len()
        );
    }
}
