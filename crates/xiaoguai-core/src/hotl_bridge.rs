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
use sqlx::SqlitePool;
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
    pool: SqlitePool,
}

impl PgHotlPolicyStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
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
            // DEC-033: single implicit owner; no per-tenant scoping.
            tenant_id: Uuid::nil(),
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
        // DEC-033: tenant_id column dropped; vestigial param ignored.
        let _ = tenant_id;
        let rows: Vec<PolicyRow> = if let Some(s) = scope {
            sqlx::query_as(
                "SELECT id, scope, window_seconds, \
                        max_count, max_usd, escalate_to \
                 FROM hotl_policies \
                 WHERE scope = ? \
                 ORDER BY created_at ASC",
            )
            .bind(s)
            .fetch_all(&self.pool)
            .await
            .map_err(pg_err)?
        } else {
            sqlx::query_as(
                "SELECT id, scope, window_seconds, \
                        max_count, max_usd, escalate_to \
                 FROM hotl_policies \
                 ORDER BY created_at ASC",
            )
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
        // DEC-033: tenant_id column dropped from hotl_policies.
        sqlx::query(
            "INSERT INTO hotl_policies \
                (id, scope, window_seconds, max_count, max_usd, escalate_to) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
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
        let result = sqlx::query("DELETE FROM hotl_policies WHERE id = ?")
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
    pool: SqlitePool,
    store: Arc<PgHotlPolicyStore>,
}

impl PgHotlEnforcer {
    #[must_use]
    pub fn new(pool: SqlitePool, store: Arc<PgHotlPolicyStore>) -> Self {
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
        // DEC-033: tenant_id column dropped from hotl_usage_log.
        if let Err(e) = sqlx::query("INSERT INTO hotl_usage_log (scope, amount) VALUES (?, ?)")
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
            // DEC-033: tenant_id dropped. SQLite date math via datetime()
            // string modifier; CAST to REAL so COUNT(*) decodes as f64.
            let row: (Option<f64>, Option<f64>) = match sqlx::query_as(
                "SELECT CAST(COUNT(*) AS REAL), CAST(SUM(amount) AS REAL) \
                 FROM hotl_usage_log \
                 WHERE scope = ? \
                   AND occurred_at >= datetime('now', '-' || ? || ' seconds')",
            )
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
//   SuspendingHotlGate     → mint a `escalation_id`, register a waiter on the
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
///
/// Sprint-13 (S13-7): an optional per-scope-class `expiry` table overrides
/// `default_expiry` on a per-call basis. The lookup is keyed on the
/// prefix of the scope before the first `.` (the "scope class" — e.g.
/// `mcp.oauth.consent` → `mcp`). Missing keys fall back to
/// `default_expiry`; an empty map preserves the v1.9.x single-knob
/// behaviour byte-for-byte. The lookup is per-call (no caching) so
/// tenants editing their config at runtime are honoured on the next
/// escalation.
pub struct SuspendingHotlGate {
    inner: Arc<dyn HotlEnforcer>,
    registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
    default_expiry: std::time::Duration,
    /// Sprint-13 S13-7: scope-class → expiry override map. Empty by
    /// default; populated from `agent.hotl.expiry` in `run_serve`.
    expiry: std::collections::HashMap<String, std::time::Duration>,
    /// Sprint-13 S13-6: per-tenant redaction rule store. The gate calls
    /// `RedactionRules::from_storage(&*redaction_repo, tenant_id)` on
    /// every escalation so admin edits land on the next call (no cache).
    /// Wiring uses a `NoopHotlRedactionRepo` shim for the sprint-12
    /// constructors that don't supply one — preserves byte-for-byte the
    /// v1.9.x behaviour.
    redaction_repo: Arc<dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo>,
    /// Sprint-13 S13-6 (S13-0 config). When `true`, an escalation with
    /// no matching redaction rule short-circuits to `Deny("redaction
    /// policy missing")` instead of leaking verbatim args on SSE. The
    /// v1.10 default is `false` (warn-once-and-pass); v1.11 flips it to
    /// `true` so unredacted tool args never reach SSE clients.
    redaction_required: bool,
    /// Sprint-13 S13-6. Optional audit sink that receives one
    /// `hotl.escalation` entry per Suspend verdict. The entry's
    /// `details` JSON embeds `redaction_policy_id` so audit queries can
    /// trace which policy masked which call. `None` skips the audit
    /// emission (acceptable for tests + boot scenarios where the audit
    /// chain is not yet wired).
    audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>>,
}

impl std::fmt::Debug for SuspendingHotlGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuspendingHotlGate")
            .field("inner", &"Arc<dyn HotlEnforcer>")
            .field("registry", &"Arc<DecisionRegistry>")
            .field("default_expiry", &self.default_expiry)
            .field("expiry_overrides", &self.expiry.len())
            .field("redaction_repo", &"Arc<dyn HotlRedactionRepo>")
            .field("redaction_required", &self.redaction_required)
            .field(
                "audit_sink",
                &self.audit_sink.as_ref().map(|_| "Arc<dyn HotlAuditSink>"),
            )
            .finish()
    }
}

/// Sprint-13 S13-6. Fallback `HotlRedactionRepo` that returns no rules.
/// Used by the sprint-12 constructors (`new`, `with_expiry`, `arc`,
/// `arc_with_expiry`) that don't accept a repo, so they keep building a
/// gate whose behaviour matches the v1.9.x pass-through.
///
/// `redaction_required = false` paired with this repo means the gate
/// warns once per instance (via `RedactionRules`) and emits verbatim
/// args. `redaction_required = true` paired with this repo would
/// fail-closed every call — only the `with_redaction` constructor (used
/// by `run_serve`) should ever combine those.
#[derive(Debug, Default, Clone)]
struct NoopHotlRedactionRepo;

#[async_trait]
impl xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo for NoopHotlRedactionRepo {
    async fn load_for_tenant(
        &self,
        _tenant_id: Uuid,
    ) -> xiaoguai_storage::repositories::error::RepoResult<
        Vec<xiaoguai_storage::repositories::hotl_redaction::RedactionPolicyRow>,
    > {
        Ok(Vec::new())
    }
}

impl SuspendingHotlGate {
    /// Construct a gate with only a default expiry (no per-scope
    /// overrides). Equivalent to `with_expiry(.., HashMap::new())`.
    /// Retained for source-compatibility with the sprint-12 wiring; new
    /// call sites should prefer [`Self::with_expiry`].
    #[must_use]
    pub fn new(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
    ) -> Self {
        Self::with_expiry(
            inner,
            registry,
            default_expiry,
            std::collections::HashMap::new(),
        )
    }

    /// Sprint-13 S13-7: construct a gate with a per-scope-class expiry
    /// table. See the struct doc for lookup semantics.
    #[must_use]
    pub fn with_expiry(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
        expiry: std::collections::HashMap<String, std::time::Duration>,
    ) -> Self {
        Self::with_redaction(
            inner,
            registry,
            default_expiry,
            expiry,
            Arc::new(NoopHotlRedactionRepo) as _,
            false,
            None,
        )
    }

    /// Sprint-13 S13-6. Construct a gate that consults `redaction_repo`
    /// on every escalation and (optionally) emits an audit entry to
    /// `audit_sink` carrying the matched `redaction_policy_id`.
    ///
    /// `redaction_required = true` enforces fail-closed semantics: any
    /// escalation that lacks a matching rule short-circuits to
    /// `HotlGateVerdict::Deny("redaction policy missing")` instead of
    /// leaking verbatim args on SSE. The v1.10 default is `false` to
    /// preserve byte-for-byte the v1.9.x pass-through; v1.11 will flip
    /// it on by default.
    ///
    /// `audit_sink = None` skips the audit emission — acceptable for
    /// tests + early-boot wiring before the audit chain is up. The
    /// `Suspend` verdict's `args_redacted` field is still populated
    /// regardless of the sink state.
    #[must_use]
    pub fn with_redaction(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
        expiry: std::collections::HashMap<String, std::time::Duration>,
        redaction_repo: Arc<dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo>,
        redaction_required: bool,
        audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>>,
    ) -> Self {
        Self {
            inner,
            registry,
            default_expiry,
            expiry,
            redaction_repo,
            redaction_required,
            audit_sink,
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
        Self::arc_with_expiry(
            inner,
            registry,
            default_expiry,
            std::collections::HashMap::new(),
        )
    }

    /// Sprint-13 S13-7: Arc helper that also threads the per-scope-class
    /// expiry table. `build_hotl_gate` uses this when the suspend gate
    /// is selected.
    #[must_use]
    pub fn arc_with_expiry(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
        expiry: std::collections::HashMap<String, std::time::Duration>,
    ) -> Arc<dyn xiaoguai_agent::HotlGate> {
        Arc::new(Self::with_expiry(inner, registry, default_expiry, expiry))
    }

    /// Sprint-13 S13-6: Arc helper that threads expiry, the redaction
    /// repo, and an audit sink. `build_hotl_gate_with_redaction` uses
    /// this when the suspend gate is selected; `run_serve` calls it
    /// directly.
    #[must_use]
    pub fn arc_with_redaction(
        inner: Arc<dyn HotlEnforcer>,
        registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
        default_expiry: std::time::Duration,
        expiry: std::collections::HashMap<String, std::time::Duration>,
        redaction_repo: Arc<dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo>,
        redaction_required: bool,
        audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>>,
    ) -> Arc<dyn xiaoguai_agent::HotlGate> {
        Arc::new(Self::with_redaction(
            inner,
            registry,
            default_expiry,
            expiry,
            redaction_repo,
            redaction_required,
            audit_sink,
        ))
    }
}

/// Sprint-13 S13-7. Resolve the expiry `Duration` for a given scope by
/// looking up the scope's class (the prefix before the first `.`) in
/// the per-scope `expiry` map. Falls back to `default_expiry` when:
///
/// * the scope has no `.` and the whole scope isn't in the map, or
/// * the scope's class isn't in the map.
///
/// Stateless on purpose: every invocation re-reads the map, so a
/// runtime config reload is honoured by the very next escalation
/// without resetting the gate.
fn resolve_expiry(
    expiry: &std::collections::HashMap<String, std::time::Duration>,
    default_expiry: std::time::Duration,
    scope: &str,
) -> std::time::Duration {
    let class = scope.split_once('.').map_or(scope, |(c, _)| c);
    expiry.get(class).copied().unwrap_or(default_expiry)
}

#[async_trait]
impl xiaoguai_agent::HotlGate for SuspendingHotlGate {
    /// Legacy entry point — preserves the v1.9.x signature so test
    /// harnesses that don't supply args continue to compile. Delegates
    /// to [`Self::check_with_args`] with `Value::Null` so the redaction
    /// path is exercised consistently (an empty input yields an empty
    /// output regardless of policy).
    async fn check(
        &self,
        tenant_id: Uuid,
        scope: &str,
        amount: f64,
    ) -> xiaoguai_agent::HotlGateVerdict {
        <Self as xiaoguai_agent::HotlGate>::check_with_args(
            self,
            tenant_id,
            scope,
            amount,
            &serde_json::Value::Null,
        )
        .await
    }

    async fn check_with_args(
        &self,
        tenant_id: Uuid,
        scope: &str,
        amount: f64,
        args: &serde_json::Value,
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
                // Sprint-13 S13-6: per-call rule load — admin edits land
                // on the next escalation (DEC-HLD-014 mutability
                // rationale). Repo failure is fail-closed: without
                // knowing the policy state we can't decide whether
                // redaction was required, so we deny rather than risk
                // leaking unredacted args on SSE.
                let rules = match xiaoguai_auth::RedactionRules::from_storage(
                    &*self.redaction_repo,
                    tenant_id,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(
                            tenant_id = %tenant_id,
                            %scope,
                            error = %e,
                            "HOTL redaction policy load failed — fail-closed deny"
                        );
                        return xiaoguai_agent::HotlGateVerdict::Deny(format!(
                            "HOTL redaction policy store error (fail-closed): {e}"
                        ));
                    }
                };

                let matching_id = rules.matching_rule_id(scope);
                // Fail-closed branch (DEC-HLD-014 + S13-0 config): if
                // redaction is required but no rule matches this scope,
                // deny. Empty rule sets and per-scope misses both surface
                // as `matching_id.is_none()` here.
                if self.redaction_required && matching_id.is_none() {
                    tracing::error!(
                        tenant_id = %tenant_id,
                        %scope,
                        "HOTL escalate but redaction_required=true and no matching policy — fail-closed deny"
                    );
                    return xiaoguai_agent::HotlGateVerdict::Deny(
                        "redaction policy missing".into(),
                    );
                }

                let args_redacted = rules.apply(scope, args);

                let escalation_id = Uuid::new_v4();
                // Sprint-13 S13-7: per-call lookup; runtime config edits
                // are honoured on the next escalation.
                let window = resolve_expiry(&self.expiry, self.default_expiry, scope);
                let expires_at_instant = tokio::time::Instant::now() + window;
                let now_utc = chrono::Utc::now();
                let expires_at_utc = now_utc
                    + chrono::Duration::from_std(window)
                        .unwrap_or_else(|_| chrono::Duration::seconds(86_400));

                // Sprint-13 S13-5: persistence-aware register. Build the
                // `hotl_escalations` parent + `hotl_pending` child rows
                // from the available gate context — the agent loop
                // doesn't surface `session_id` through the trait, so we
                // use the escalation_id as a synthetic session anchor
                // (sprint-13 OOS: full session threading lands in S13-8).
                let parent = xiaoguai_storage::repositories::hotl_escalations::HotlEscalationRow {
                    id: escalation_id,
                    tenant_id,
                    session_id: escalation_id,
                    top_level_scope: scope.to_string(),
                    status: "pending".to_string(),
                    created_at: now_utc,
                    parent_id: None,
                };
                let child = xiaoguai_storage::repositories::hotl_escalations::HotlPendingRow {
                    id: Uuid::new_v4(),
                    escalation_id,
                    tenant_id,
                    scope: scope.to_string(),
                    // Sprint-13 S13-8 will plumb the tool name through;
                    // the scope already encodes it (`tool_call.<name>`).
                    tool: scope.to_string(),
                    // Sprint-13 S13-6: persist the policy-masked args
                    // so a UI restart restores the same redacted view
                    // that the live SSE banner displayed.
                    args_redacted: args_redacted.clone(),
                    status: "pending".to_string(),
                    expires_at: expires_at_utc,
                    created_at: now_utc,
                    decided_at: None,
                    decided_by: None,
                };

                match self
                    .registry
                    .register_persisted(escalation_id, parent, child, expires_at_instant)
                    .await
                {
                    Ok(ticket) => {
                        tracing::info!(
                            tenant_id = %tenant_id,
                            %scope,
                            %reason,
                            %escalation_id,
                            ?matching_id,
                            "HOTL escalate → suspend; awaiting operator decision"
                        );

                        // Sprint-13 S13-6: emit one `hotl.escalation`
                        // audit entry per Suspend verdict, embedding
                        // the matched `redaction_policy_id` so audit
                        // queries can trace policy lineage. Audit
                        // failure is logged but does NOT block the
                        // operator decision flow.
                        if let Some(sink) = &self.audit_sink {
                            let entry = xiaoguai_audit::AuditEntry {
                                ts: now_utc,
                                tenant_id: tenant_id.to_string(),
                                actor: "system".into(),
                                action: "hotl.escalation".into(),
                                resource: Some(format!("escalation:{escalation_id}")),
                                details: serde_json::json!({
                                    "scope": scope,
                                    "redaction_policy_id": matching_id,
                                }),
                            };
                            if let Err(e) = sink.append(entry).await {
                                tracing::warn!(
                                    tenant_id = %tenant_id,
                                    %scope,
                                    %escalation_id,
                                    error = %e,
                                    "HOTL escalation audit append failed — continuing"
                                );
                            }
                        }

                        xiaoguai_agent::HotlGateVerdict::Suspend {
                            escalation_id,
                            scope: scope.to_string(),
                            ticket,
                            args_redacted,
                        }
                    }
                    Err(e) => {
                        // Sprint-13 S13-5: persist failure is fail-closed.
                        // Without a persisted row, boot replay can't
                        // resurrect the waiter and the operator UI has
                        // no escalation record to act on. Deny the tool
                        // call rather than leaving a phantom in-memory
                        // waiter (the persisted-first ordering guarantees
                        // no waiter exists at this point).
                        tracing::error!(
                            tenant_id = %tenant_id,
                            %scope,
                            error = %e,
                            %escalation_id,
                            "HOTL escalate persistence failed — fail-closed deny"
                        );
                        xiaoguai_agent::HotlGateVerdict::Deny(format!(
                            "HOTL escalation persistence error (fail-closed): {e}"
                        ))
                    }
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
    build_hotl_gate_with_expiry(
        suspend_on_escalate,
        enforcer,
        registry,
        default_expiry,
        std::collections::HashMap::new(),
    )
}

/// Sprint-13 S13-7. `build_hotl_gate` variant that also threads the
/// per-scope-class expiry map into `SuspendingHotlGate`. The map is
/// ignored when `suspend_on_escalate == false` (the legacy
/// `EnforcerGate` has no suspend window to override). `run_serve` calls
/// this directly; the older 4-arg signature delegates here with an
/// empty map for source-compatibility with sprint-12 test fixtures.
#[must_use]
pub fn build_hotl_gate_with_expiry(
    suspend_on_escalate: bool,
    enforcer: Arc<dyn HotlEnforcer>,
    registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
    default_expiry: std::time::Duration,
    expiry: std::collections::HashMap<String, std::time::Duration>,
) -> Arc<dyn xiaoguai_agent::HotlGate> {
    // Sprint-13 S13-6: delegate to the redaction-aware builder with a
    // Noop repo + redaction_required=false + no audit sink, so existing
    // test fixtures keep observing byte-for-byte the sprint-12
    // behaviour (verbatim args, no policy lookup).
    build_hotl_gate_with_redaction(
        suspend_on_escalate,
        enforcer,
        registry,
        default_expiry,
        expiry,
        Arc::new(NoopHotlRedactionRepo) as _,
        false,
        None,
    )
}

/// Sprint-13 S13-6. `build_hotl_gate_with_expiry` variant that also
/// threads the redaction repo + `redaction_required` flag + audit sink
/// into `SuspendingHotlGate`. Ignored when `suspend_on_escalate ==
/// false` (legacy `EnforcerGate` does not have a redaction surface).
/// `run_serve` calls this directly with `PgHotlRedactionRepo` and the
/// `agent.hotl.redaction_policy_required` config field.
#[must_use]
pub fn build_hotl_gate_with_redaction(
    suspend_on_escalate: bool,
    enforcer: Arc<dyn HotlEnforcer>,
    registry: Arc<xiaoguai_api::hotl::decision_registry::DecisionRegistry>,
    default_expiry: std::time::Duration,
    expiry: std::collections::HashMap<String, std::time::Duration>,
    redaction_repo: Arc<dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo>,
    redaction_required: bool,
    audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>>,
) -> Arc<dyn xiaoguai_agent::HotlGate> {
    if suspend_on_escalate {
        SuspendingHotlGate::arc_with_redaction(
            enforcer,
            registry,
            default_expiry,
            expiry,
            redaction_repo,
            redaction_required,
            audit_sink,
        )
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
// `escalation_id` is the idempotency guard — a duplicate insert surfaces as
// `HotlDecisionStoreError::Duplicate(escalation_id)` so the route returns 409.
//
// `verdict` is stored as the lowercase text the SQL CHECK constraint enforces
// (`'allow' | 'deny'`); on read we map both values back into the
// `HotlDecisionVerdict` enum. Any other value (impossible given the CHECK)
// surfaces as a `Backend` error rather than silently coercing.

#[derive(Debug, Clone)]
pub struct PgHotlDecisionStore {
    pool: SqlitePool,
}

impl PgHotlDecisionStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Box-and-Arc helper so callers in `run_serve` don't repeat the dyn coercion.
    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<dyn HotlDecisionStore> {
        Arc::new(Self::new(pool))
    }
}

fn decision_pg_err(e: sqlx::Error) -> HotlDecisionStoreError {
    HotlDecisionStoreError::Other(e.to_string())
}

#[async_trait]
impl HotlDecisionStore for PgHotlDecisionStore {
    async fn record(
        &self,
        escalation_id: Uuid,
        tenant_id: Uuid,
        verdict: HotlDecisionVerdict,
        decided_by: String,
        raised_policy_id: Option<Uuid>,
    ) -> Result<HotlDecisionRecord, HotlDecisionStoreError> {
        // DEC-033: `hotl_decisions` has no tenant_id column and the
        // idempotency column is `request_id` (UNIQUE). We bind the
        // `escalation_id` value into `request_id`. The vestigial
        // `tenant_id` param is ignored.
        let _ = tenant_id;
        let id = Uuid::new_v4();

        let row: (Uuid, Uuid, String, String, Option<Uuid>, DateTime<Utc>) = sqlx::query_as(
            "INSERT INTO hotl_decisions \
                    (id, request_id, verdict, decided_by, raised_policy_id) \
                 VALUES (?, ?, ?, ?, ?) \
                 RETURNING id, request_id, verdict, decided_by, raised_policy_id, recorded_at",
        )
        .bind(id)
        .bind(escalation_id)
        .bind(verdict.as_str())
        .bind(&decided_by)
        .bind(raised_policy_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            // SQLite surfaces unique violations as a database error whose
            // message contains "UNIQUE constraint failed".
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.message().contains("UNIQUE constraint failed") {
                    return HotlDecisionStoreError::Duplicate(escalation_id);
                }
            }
            decision_pg_err(e)
        })?;

        let returned_verdict = match row.2.as_str() {
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
            // DEC-033: single implicit owner.
            tenant_id: Uuid::nil(),
            verdict: returned_verdict,
            decided_by: row.3,
            raised_policy_id: row.4,
            recorded_at: row.5,
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

    // ── Sprint-13 S13-7: resolve_expiry helper ────────────────────────────────

    #[test]
    fn resolve_expiry_uses_class_match_when_present() {
        let mut expiry = std::collections::HashMap::new();
        expiry.insert("mcp".to_string(), std::time::Duration::from_secs(4 * 3600));
        let default_expiry = std::time::Duration::from_secs(24 * 3600);
        let got = resolve_expiry(&expiry, default_expiry, "mcp.oauth.consent");
        assert_eq!(got, std::time::Duration::from_secs(4 * 3600));
    }

    #[test]
    fn resolve_expiry_falls_back_to_default_when_class_missing() {
        let mut expiry = std::collections::HashMap::new();
        expiry.insert("mcp".to_string(), std::time::Duration::from_secs(4 * 3600));
        let default_expiry = std::time::Duration::from_secs(24 * 3600);
        let got = resolve_expiry(&expiry, default_expiry, "tool_call.execute_python");
        assert_eq!(got, default_expiry);
    }

    #[test]
    fn resolve_expiry_falls_back_on_malformed_scope() {
        // No '.' in the scope; class is the whole string. The empty key
        // exists in the map but the scope class is `weird`, not `""`,
        // so the lookup misses and falls back to default.
        let mut expiry = std::collections::HashMap::new();
        expiry.insert(String::new(), std::time::Duration::from_secs(1));
        expiry.insert("mcp".to_string(), std::time::Duration::from_secs(4 * 3600));
        let default_expiry = std::time::Duration::from_secs(24 * 3600);
        let got = resolve_expiry(&expiry, default_expiry, "weird");
        assert_eq!(got, default_expiry);
    }

    #[test]
    fn resolve_expiry_with_empty_map_returns_default() {
        let expiry = std::collections::HashMap::new();
        let default_expiry = std::time::Duration::from_secs(24 * 3600);
        let got = resolve_expiry(&expiry, default_expiry, "tool_call.search");
        assert_eq!(got, default_expiry);
    }

    #[test]
    fn resolve_expiry_matches_scope_without_dot_when_class_present() {
        // Scope == class (no dot suffix). The whole scope is the class.
        let mut expiry = std::collections::HashMap::new();
        expiry.insert("mcp".to_string(), std::time::Duration::from_secs(4 * 3600));
        let default_expiry = std::time::Duration::from_secs(24 * 3600);
        let got = resolve_expiry(&expiry, default_expiry, "mcp");
        assert_eq!(got, std::time::Duration::from_secs(4 * 3600));
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

    // ── SQLite integration tests (DEC-033) ────────────────────────────────────

    async fn sqlite_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
            .await
            .unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn hotl_store_create_list_delete() {
        let (_dir, pool) = sqlite_pool().await;
        let store = PgHotlPolicyStore::new(pool);
        // tenant_id is vestigial under DEC-033 (single owner).
        let tid = Uuid::nil();

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

        // Scoped filter still works (scope column survives DEC-033).
        let scoped = store.list(tid, Some("llm_call")).await.unwrap();
        assert_eq!(scoped.len(), 1);
        let empty = store.list(tid, Some("email_send")).await.unwrap();
        assert!(empty.is_empty());

        store.delete(created.id).await.unwrap();
        let after = store.list(tid, None).await.unwrap();
        assert!(after.iter().all(|p| p.id != created.id));
    }

    #[tokio::test]
    async fn hotl_store_delete_missing_is_not_found() {
        let (_dir, pool) = sqlite_pool().await;
        let store = PgHotlPolicyStore::new(pool);
        let err = store.delete(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(
            err,
            xiaoguai_api::hotl::policy::HotlPolicyStoreError::NotFound(_)
        ));
    }

    // DELETED hotl_pg_store_tenant_isolation: under DEC-033 there is one
    // implicit owner; `list` ignores tenant_id and returns all rows, so
    // per-tenant isolation is no longer a meaningful behaviour to assert.

    #[tokio::test]
    async fn hotl_enforcer_no_policy_allows() {
        let (_dir, pool) = sqlite_pool().await;
        let store = Arc::new(PgHotlPolicyStore::new(pool.clone()));
        let enforcer = PgHotlEnforcer::new(pool, store);
        let v = enforcer.check(Uuid::nil(), "llm_call", 1.0).await.unwrap();
        assert_eq!(v, HotlVerdict::Allow);
    }

    #[tokio::test]
    async fn hotl_enforcer_count_breach_denies() {
        let (_dir, pool) = sqlite_pool().await;
        let store = Arc::new(PgHotlPolicyStore::new(pool.clone()));
        let tid = Uuid::nil();

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
        let (escalation_id, scope, ticket) = match v {
            xiaoguai_agent::HotlGateVerdict::Suspend {
                escalation_id,
                scope,
                ticket,
                args_redacted: _,
            } => (escalation_id, scope, ticket),
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
            escalation_id,
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
        let (new_escalation_id, _scope, _ticket) = match v {
            xiaoguai_agent::HotlGateVerdict::Suspend {
                escalation_id,
                scope,
                ticket,
                args_redacted: _,
            } => (escalation_id, scope, ticket),
            other => panic!("expected Suspend, got {other:?}"),
        };
        assert_ne!(new_escalation_id, preexisting_id);
        assert_eq!(
            reg.len(),
            2,
            "shared registry must hold both waiters; got {} (gate likely minted its own registry)",
            reg.len()
        );
    }
}
