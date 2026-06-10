//! Integration coverage for sprint-13 S13-8 (DEC-HLD-016): clean
//! `request_id` → `escalation_id` rename across the `HotL` surface, with
//! NO backward-compat alias.
//!
//! Pins:
//!
//! 1. `POST`ing a body with the legacy `request_id` key returns
//!    `400 Bad Request` with a structured `{error:"field", field:"escalation_id", ...}`
//!    payload that tells the client how to migrate.
//! 2. `POST`ing with the canonical `escalation_id` key returns 201 + a body
//!    that uses `escalation_id` (NOT `request_id`).
//! 3. SSE-encoded `hotl_pending` events use the field name `escalation_id`
//!    on the wire (and crucially do not include `request_id`).

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, AgentEvent, HotlResolution, Toolbox};
use xiaoguai_api::hotl::decision::{HotlDecisionStore, InMemoryHotlDecisionStore};
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::sse::event_to_sse;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(decision_store: Arc<dyn HotlDecisionStore>) -> AppState {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        audit: None,
        audit_verifier: None,
        audit_chain_exporter: None,
        mcp_publish_enabled: false,
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        usage_reader: None,
        session_forker: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        hotl_decision_store: Some(decision_store),
        hotl_audit: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers: None,
        loops: None,
        teams: None,
        team_audit: None,
        decision_registry: Arc::new(DecisionRegistry::new()),
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── 1. Legacy `request_id` key rejected with structured 400 ──────────────────

#[tokio::test]
async fn decision_with_legacy_request_id_returns_400() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(decisions));

    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "ops@acme.com"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "legacy `request_id` body must be rejected with 400 (no compat alias per DEC-HLD-016)"
    );

    let json = body_json(resp.into_body()).await;
    // The error body must point the operator at the new field name so
    // they can update their client without digging through release notes.
    assert_eq!(json["field"], "escalation_id");
    let message = json["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("escalation_id") && message.contains("request_id"),
        "error message must mention both the old and new field names: {message}"
    );
}

// ── 2. Canonical `escalation_id` parses and surfaces in the response ─────────

#[tokio::test]
async fn decision_with_escalation_id_returns_201() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(decisions));

    let escalation_id = Uuid::new_v4();
    let body = serde_json::json!({
        "escalation_id": escalation_id.to_string(),
        "verdict": "allow",
        "decided_by": "ops@acme.com"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let json = body_json(resp.into_body()).await;
    assert_eq!(json["escalation_id"], escalation_id.to_string());
    assert!(
        json.get("request_id").is_none(),
        "response body must NOT include legacy `request_id` field: {json}"
    );
}

// ── 3. SSE `hotl_pending` event uses `escalation_id` on the wire ─────────────

#[tokio::test]
async fn sse_event_payload_uses_escalation_id() {
    use chrono::{TimeZone, Utc};
    let escalation_id = Uuid::new_v4();
    let ev = AgentEvent::HotlPending {
        escalation_id,
        tool: "execute_python".into(),
        args_redacted: serde_json::json!({"code": "[redacted]"}),
        scope: "tool_call.execute_python".into(),
        expires_at: Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap(),
    };
    let sse = event_to_sse(&ev);
    let rendered = format!("{sse:?}");
    assert!(
        rendered.contains("escalation_id"),
        "SSE payload must include the canonical field name `escalation_id`: {rendered}"
    );
    assert!(
        !rendered.contains("request_id"),
        "SSE payload must NOT include the legacy field name `request_id`: {rendered}"
    );
    assert!(
        rendered.contains(&escalation_id.to_string()),
        "SSE payload must include the actual uuid: {rendered}"
    );
}

// ── 4. SSE `hotl_resolved` event uses `escalation_id` on the wire ────────────

#[tokio::test]
async fn sse_hotl_resolved_payload_uses_escalation_id() {
    use chrono::{TimeZone, Utc};
    let escalation_id = Uuid::new_v4();
    let ev = AgentEvent::HotlResolved {
        escalation_id,
        verdict: HotlResolution::Allow,
        decided_by: Some("ops@acme.com".into()),
        recorded_at: Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 5).unwrap(),
    };
    let sse = event_to_sse(&ev);
    let rendered = format!("{sse:?}");
    assert!(
        rendered.contains("escalation_id"),
        "SSE payload must include the canonical field name `escalation_id`: {rendered}"
    );
    assert!(
        !rendered.contains("request_id"),
        "SSE payload must NOT include the legacy field name `request_id`: {rendered}"
    );
}
