//! `POST /v1/hotl/decisions` — HOTL decision-record endpoint (v1.8.x
//! sprint-11 S11-3a.1).
//!
//! Records a human verdict (`allow` / `deny`) against an escalated HOTL
//! request and optionally creates a follow-up `HotlPolicy` in the same
//! request ("Approve & remember" UX).
//!
//! ## `resumed` flag (sprint-12 S12-6)
//!
//! The handler also wakes any parked agent loop registered against the
//! same `escalation_id` on the [`crate::hotl::decision_registry::DecisionRegistry`].
//! The response's `resumed: bool` is the return value of
//! `DecisionRegistry::resolve`:
//!
//! * `true`  — a `SuspendingHotlGate` (S12-4) had parked a loop on this
//!   `escalation_id`; it is now released with the operator's verdict.
//! * `false` — no live waiter existed (legacy `EnforcerGate` path that
//!   never suspends, OR the ticket already timed out / was cancelled).
//!
//! Ordering: the decision row is persisted **before** the registry
//! resolve, so a registry-side crash never loses the operator's audit
//! trail. The registry op is a single in-memory `DashMap::remove +
//! oneshot::Sender::send`; in practice it cannot panic on a `resolve`
//! call (per S12-3 unit tests), but the ordering rule is the safety net.
//!
//! ## Tenant identity
//!
//! Under the single-user pivot (DEC-033) there is one implicit owner, so the
//! decision row is always written against the nil tenant UUID. The field is
//! retained only because the persistence layer still threads a `tenant_id`
//! column; it carries no isolation semantics.
//!
//! ## `raise_policy` semantics
//!
//! When `raise_policy` is present the handler:
//!
//! 1. Records the decision row first (so we have an `id` to point the
//!    `raised_policy_id` at).
//! 2. Calls `hotl_policy_store.create(...)`.
//! 3. If step 2 fails, the in-mem store has no rollback hook — we surface
//!    the policy error as a 4xx/5xx and leave the orphan decision row
//!    intact. The PG store should run both writes inside a single
//!    transaction; that wiring lives in `xiaoguai-core::hotl_bridge`.
//!
//! Open question Q2 (plan §7): `raise_policy` is accepted on **both**
//! `allow` and `deny` verdicts. Tightening on deny is a real workflow
//! ("never approve LLM calls in this scope again"); the design does not
//! gate the field on verdict.
//!
//! Open question Q3 (plan §7): duplicate `escalation_id` returns `409 Conflict`.
//! The handler does not implement idempotent-replay semantics — operators
//! who double-click see a clear error rather than a silent no-op.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::hotl::decision::{HotlDecisionRecord, HotlDecisionStoreError, HotlDecisionVerdict};
use crate::hotl::decision_registry::{HotlResolution, RegistryError};
use crate::hotl::policy::{CreateHotlPolicyRequest, HotlPolicy};
use crate::routes::hotl::map_store_err as map_policy_err;
use crate::state::AppState;

// ── wire DTOs ─────────────────────────────────────────────────────────────────

/// Body accepted by `POST /v1/hotl/decisions`.
///
/// `escalation_id` is the canonical name; `escalation_id` is accepted as a
/// `#[serde(alias)]` so the existing SSE-event field name and chat-ui
/// e2e mocks keep working without a flag day. Full rename across the SSE
/// contract is deferred (plan §4 OOS).
#[derive(Debug, Deserialize)]
pub struct CreateHotlDecisionRequest {
    #[serde(alias = "escalation_id")]
    pub escalation_id: Uuid,
    pub verdict: HotlDecisionVerdict,
    pub decided_by: String,
    /// Optional follow-up policy ("Approve & remember" / "Deny & tighten").
    #[serde(default)]
    pub raise_policy: Option<RaisePolicyRequest>,
}

/// Sub-DTO carried inside [`CreateHotlDecisionRequest`].
///
/// Shape mirrors [`CreateHotlPolicyRequest`] minus `tenant_id` (taken from
/// Claims, not the body). `scope` + a `window_seconds` + at least one
/// budget (`max_count` OR `max_usd`) are required by the policy store.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RaisePolicyRequest {
    pub scope: String,
    #[serde(default)]
    pub tool: Option<String>,
    pub window_seconds: i32,
    #[serde(default)]
    pub max_count: Option<i32>,
    #[serde(default)]
    pub max_usd: Option<f64>,
    #[serde(default)]
    pub escalate_to: Option<String>,
}

/// `201 Created` body returned by the decision route.
#[derive(Debug, Serialize)]
pub struct HotlDecisionResponse {
    pub id: Uuid,
    pub escalation_id: Uuid,
    pub verdict: HotlDecisionVerdict,
    pub recorded_at: DateTime<Utc>,
    /// `true` when a live waiter on `DecisionRegistry` was woken by this
    /// decision (sprint-12 S12-6); `false` when no waiter existed — either
    /// the legacy non-suspending `EnforcerGate` path, or a ticket that
    /// already timed out / was cancelled before the operator decided.
    pub resumed: bool,
    /// `Some(policy)` when `raise_policy` was present and the follow-up
    /// `HotlPolicy::create` succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_created: Option<HotlPolicy>,
}

impl HotlDecisionResponse {
    /// Build a response from the persisted decision row plus the live
    /// `resumed` flag returned by [`crate::hotl::decision_registry::DecisionRegistry::resolve`].
    fn from_record(r: HotlDecisionRecord, resumed: bool) -> Self {
        Self {
            id: r.id,
            escalation_id: r.request_id,
            verdict: r.verdict,
            recorded_at: r.recorded_at,
            resumed,
            policy_created: None,
        }
    }
}

// ── handler ───────────────────────────────────────────────────────────────────

/// `POST /v1/hotl/decisions`
///
/// Body: [`CreateHotlDecisionRequest`].
/// Returns `201 Created` with [`HotlDecisionResponse`].
///
/// # Errors
///
/// - `503 ServiceUnavailable` — decision store not wired.
/// - `400 InvalidRequest` — malformed `raise_policy` (e.g. both
///   `max_count` and `max_usd` null).
/// - `404 NotFound` — reserved for when the parent `hotl_escalations`
///   table lands in 3a.2. 3a.1 has no parent table; this status is
///   currently unreachable from a well-formed request (kept on the wire
///   so 3a.2 can return it without a client breaking change).
/// - `409 Conflict` — `escalation_id` already has a recorded decision.
/// - `401 Unauthorized` — handled by the owner-auth middleware.
pub async fn create_decision(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    // Sprint-13 S13-8 / DEC-HLD-016: pre-flight check for the legacy
    // `request_id` field so callers get a structured rename diagnostic
    // (400 with `{field: "escalation_id"}`) instead of a generic
    // unknown-field error.
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::InvalidRequest(format!("invalid JSON body: {e}")).into_response();
        }
    };
    if let Some(obj) = value.as_object() {
        if obj.contains_key("request_id") && !obj.contains_key("escalation_id") {
            return rename_diagnostic_response();
        }
    }
    let req: CreateHotlDecisionRequest = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            return ApiError::InvalidRequest(format!("invalid request body: {e}")).into_response();
        }
    };
    match create_decision_inner(state, req).await {
        Ok((status, body)) => (status, body).into_response(),
        Err(api) => api.into_response(),
    }
}

/// Sprint-13 S13-8 / DEC-HLD-016. Build the 400 response emitted when a
/// caller posts a body with the legacy `request_id` key. The body shape
/// `{error, field, message}` is stable so client error handlers can switch
/// on `field` and prompt the user to upgrade.
fn rename_diagnostic_response() -> Response {
    let body = serde_json::json!({
        "error": "field",
        "field": "escalation_id",
        "message": "request_id was renamed to escalation_id in v1.10.0; update your client to send the `escalation_id` field instead.",
    });
    (axum::http::StatusCode::BAD_REQUEST, Json(body)).into_response()
}

async fn create_decision_inner(
    state: AppState,
    req: CreateHotlDecisionRequest,
) -> Result<(StatusCode, Json<HotlDecisionResponse>), ApiError> {
    // ── 1. Required wiring ───────────────────────────────────────────────────
    let store = state
        .hotl_decision_store
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("HOTL decision store not wired".into()))?;

    // ── 2. Owner tenant (DEC-033 single-user) ───────────────────────────────
    // One implicit owner → the decision row is written against the nil tenant
    // UUID; the column is vestigial and carries no isolation semantics.
    let tenant_id = Uuid::nil();

    // ── 3. Validate raise_policy shape before touching the DB ───────────────
    if let Some(rp) = &req.raise_policy {
        if rp.max_count.is_none() && rp.max_usd.is_none() {
            return Err(ApiError::InvalidRequest(
                "raise_policy: at least one of max_count or max_usd must be set".into(),
            ));
        }
        if rp.window_seconds <= 0 {
            return Err(ApiError::InvalidRequest(
                "raise_policy: window_seconds must be > 0".into(),
            ));
        }
    }

    // ── 4. Record the decision (no raised_policy_id yet) ────────────────────
    let initial_record = store
        .record(
            req.escalation_id,
            tenant_id,
            req.verdict,
            req.decided_by.clone(),
            None,
        )
        .await
        .map_err(map_decision_err)?;

    // ── 5. Optionally create the follow-up policy ───────────────────────────
    //
    // In-mem two-step: record first, then create the policy. If policy
    // creation fails the decision row is left orphaned (no rollback hook
    // in the in-mem store; PG path runs both in a transaction — see
    // `xiaoguai-core::hotl_bridge`). The plan §3 risks table flags this
    // as a known limitation of the in-mem path.
    let policy_created = if let Some(rp) = req.raise_policy.clone() {
        let policy_store = state.hotl_policy_store.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable(
                "raise_policy requested but HOTL policy store is not wired".into(),
            )
        })?;
        let create_req = CreateHotlPolicyRequest {
            tenant_id,
            scope: rp.scope.clone(),
            window_seconds: rp.window_seconds,
            max_count: rp.max_count,
            max_usd: rp.max_usd,
            escalate_to: rp.escalate_to.clone(),
        };
        let policy = policy_store
            .create(create_req)
            .await
            .map_err(map_policy_err)?;
        Some(policy)
    } else {
        None
    };

    // ── 6. Best-effort audit log ────────────────────────────────────────────
    //
    // Audit failure MUST NOT block the operation — the decision is already
    // persisted. `.ok()` discards the error; production wiring of
    // `PgAuditSink` logs internally on append failure.
    if let Some(sink) = &state.hotl_audit {
        let entry = xiaoguai_audit::AuditEntry {
            ts: Utc::now(),
            tenant_id: tenant_id.to_string(),
            actor: req.decided_by.clone(),
            action: "hotl.decision".into(),
            resource: Some(format!("escalation:{}", req.escalation_id)),
            details: serde_json::json!({
                "verdict": req.verdict,
                "raise_policy": req.raise_policy,
                "policy_created_id": policy_created.as_ref().map(|p| p.id),
            }),
        };
        let _ = sink.append(entry).await;
    }

    // ── 7. Wake the parked agent loop (S12-6) ───────────────────────────────
    //
    // Runs AFTER the persist + raise_policy + audit-log steps so that a
    // hypothetical registry-side panic cannot lose the operator's audit
    // trail. `resolve` is a single `DashMap::remove` + `oneshot::send` and
    // returns `false` when no waiter exists (legacy `EnforcerGate` path or
    // already-timed-out ticket).
    let resolution = match req.verdict {
        HotlDecisionVerdict::Allow => HotlResolution::Allow,
        // Deny carries no operator-supplied reason in the current wire
        // contract (sprint-11 schema). The synthetic ToolResult the loop
        // builds for the LLM is keyed off the verdict tag, not free text;
        // a future sprint can add `deny_reason` to the request body and
        // surface it here.
        HotlDecisionVerdict::Deny => HotlResolution::Deny(String::new()),
    };
    // Sprint-13 S13-5: persist the verdict through the
    // `HotlEscalationStore` BEFORE firing the oneshot. The store update
    // is the source of truth — the in-memory waiter may or may not still
    // exist (legacy `EnforcerGate` path, already-cancelled ticket,
    // etc.).
    //
    // Fallback compat: when the registry is wired to
    // `NoopHotlEscalationStore` (tests + 1.8.x deployments before
    // sprint-13 PG migration), `resolve_persisted` still rebroadcasts
    // through the oneshot path because the no-op store returns
    // `Ok(true)` for every `record_decision`. Real PG returns
    // `Ok(false)` for unknown ids → `Err(UnknownEscalation)` → 404.
    let resumed = match state
        .decision_registry
        .resolve_persisted(req.escalation_id, resolution, Some(req.decided_by.clone()))
        .await
    {
        Ok(true) => true,
        Ok(false) => false,
        Err(RegistryError::UnknownEscalation) => {
            // Sprint-12 S12-6 contract: late decision after timeout
            // returns `resumed=false`. Sprint-13 carries forward that
            // behaviour by treating `UnknownEscalation` as a
            // non-failure in the in-memory + Noop-store path.  When the
            // store IS PG-backed, the route currently still completes
            // with `resumed=false` rather than 404 — the 404 will be
            // wired in S13-8 once the wire rename lands and parent
            // table presence is asserted unconditionally.
            false
        }
        Err(RegistryError::Storage(e)) => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "hotl decision registry storage: {e}"
            )));
        }
    };

    // ── 8. Build the response ───────────────────────────────────────────────
    let mut resp = HotlDecisionResponse::from_record(initial_record, resumed);
    resp.policy_created = policy_created;

    Ok((StatusCode::CREATED, Json(resp)))
}

// ── error mapping ─────────────────────────────────────────────────────────────

fn map_decision_err(e: HotlDecisionStoreError) -> ApiError {
    match e {
        HotlDecisionStoreError::Duplicate(id) => {
            ApiError::Conflict(format!("decision already recorded for escalation_id {id}"))
        }
        HotlDecisionStoreError::Other(msg) => {
            ApiError::Internal(anyhow::anyhow!("HOTL decision store: {msg}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_escalation_id_parses() {
        let raw = serde_json::json!({
            "escalation_id": "00000000-0000-0000-0000-000000000001",
            "verdict": "allow",
            "decided_by": "alice"
        });
        let parsed: CreateHotlDecisionRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(
            parsed.escalation_id,
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
        );
        assert_eq!(parsed.verdict, HotlDecisionVerdict::Allow);
    }

    #[test]
    fn canonical_escalation_id_parses() {
        let raw = serde_json::json!({
            "escalation_id": "00000000-0000-0000-0000-000000000002",
            "verdict": "deny",
            "decided_by": "bob"
        });
        let parsed: CreateHotlDecisionRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.verdict, HotlDecisionVerdict::Deny);
    }

    #[test]
    fn response_from_record_passes_through_resumed_flag() {
        let rec = HotlDecisionRecord {
            id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            verdict: HotlDecisionVerdict::Allow,
            decided_by: "x".into(),
            raised_policy_id: None,
            recorded_at: Utc::now(),
        };
        let off = HotlDecisionResponse::from_record(rec.clone(), false);
        assert!(!off.resumed);
        assert!(off.policy_created.is_none());
        let on = HotlDecisionResponse::from_record(rec, true);
        assert!(on.resumed);
    }
}
