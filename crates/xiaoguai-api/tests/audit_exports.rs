//! T5 — coverage for `POST /v1/audit/exports`.
//!
//! Uses `StaticAuditChainExporter` to avoid standing a database. The
//! happy-path / chain-broken / 503 / 501 status-code mapping is exercised
//! here; the chain-verify-and-render logic itself is covered in
//! `xiaoguai-audit` integration tests.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use chrono::{TimeZone, Utc};
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::audit::{
    AuditChainExporter, ExportError as ApiExportError, StaticAuditChainExporter,
};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(exporter: Option<Arc<dyn AuditChainExporter>>) -> AppState {
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
        audit_chain_exporter: exporter,
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
        hotl_decision_store: None,
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
        skills_dir: std::env::temp_dir(),
        personas: None,
        watchers: None,
        loops: None,
        teams: None,
        incidents: None,
        team_audit: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
        pack_rescanner: None,
        coding_toolbox_factory: None,
    }
}

fn request_body() -> Value {
    json!({
        "framework": "soc2",
        "format": "json",
        "from": Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap().to_rfc3339(),
        "to": Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap().to_rfc3339(),
    })
}

async fn post(app: axum::Router, body: Value) -> (StatusCode, Vec<u8>, Option<String>) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audit/exports")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, bytes.to_vec(), content_type)
}

#[tokio::test]
async fn returns_503_when_exporter_not_wired() {
    let app = router(build_state(None));
    let (status, _body, _ct) = post(app, request_body()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn happy_path_returns_bundle_bytes_with_correct_content_type() {
    let canned = br#"{"header":{"framework":"soc2-cc72"},"rows":[]}"#.to_vec();
    let exporter =
        Arc::new(StaticAuditChainExporter::new().with("soc2", "json", Ok(canned.clone())));
    let app = router(build_state(Some(exporter)));
    let (status, body, ct) = post(app, request_body()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("application/json"));
    assert_eq!(body, canned);
}

#[tokio::test]
async fn csv_response_uses_text_csv_content_type() {
    let canned = b"# bundle-header: {}\r\nid,ts,actor,action,resource,details_summary\r\n".to_vec();
    let exporter =
        Arc::new(StaticAuditChainExporter::new().with("soc2", "csv", Ok(canned.clone())));
    let app = router(build_state(Some(exporter)));
    let mut body = request_body();
    body["format"] = json!("csv");
    let (status, body, ct) = post(app, body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("text/csv"));
    assert!(body.starts_with(b"# bundle-header"));
}

#[tokio::test]
async fn chain_broken_returns_409_with_structured_body() {
    let broken_ts = Utc.with_ymd_and_hms(2026, 2, 15, 12, 30, 0).unwrap();
    let exporter = Arc::new(StaticAuditChainExporter::new().with(
        "soc2",
        "json",
        Err(ApiExportError::ChainBroken {
            first_broken_id: 42,
            first_broken_ts: broken_ts,
        }),
    ));
    let app = router(build_state(Some(exporter)));
    let (status, body, _ct) = post(app, request_body()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"].as_str(), Some("chain_broken"));
    assert_eq!(v["first_broken_id"].as_i64(), Some(42));
    assert!(v["first_broken_ts"].is_string());
}

#[tokio::test]
async fn pdf_format_returns_501_not_implemented() {
    let exporter = Arc::new(StaticAuditChainExporter::new().with(
        "soc2",
        "pdf",
        Err(ApiExportError::PdfUnimplemented),
    ));
    let app = router(build_state(Some(exporter)));
    let mut body = request_body();
    body["format"] = json!("pdf");
    let (status, body, _ct) = post(app, body).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"].as_str(), Some("pdf_unimplemented"));
}

#[tokio::test]
async fn export_uses_owner_tenant() {
    // DEC-033 single-owner: the export always runs against the owner tenant
    // (no wire `tenant_id`); the route keys the exporter by the audit OWNER.
    let exporter =
        Arc::new(StaticAuditChainExporter::new().with("soc2", "json", Ok(b"{}".to_vec())));
    let app = router(build_state(Some(exporter)));
    let body = request_body();
    let (status, _body, _ct) = post(app, body).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn inverted_window_returns_400() {
    let exporter = Arc::new(StaticAuditChainExporter::new());
    let app = router(build_state(Some(exporter)));
    let mut body = request_body();
    // Swap from/to.
    let from = body["from"].clone();
    body["from"] = body["to"].clone();
    body["to"] = from;
    let (status, _body, _ct) = post(app, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_format_returns_400() {
    let exporter = Arc::new(StaticAuditChainExporter::new());
    let app = router(build_state(Some(exporter)));
    let mut body = request_body();
    body["format"] = json!("xml");
    let (status, _body, _ct) = post(app, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
