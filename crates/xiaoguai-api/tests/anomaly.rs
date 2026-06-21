//! Integration tests for the anomaly-monitor REST surface
//! (`POST /v1/anomaly/test` + `POST /v1/anomaly/run`).
//!
//! Exercises the real `router()` so the wire contract the
//! `xiaoguai anomaly` CLI depends on — response field names `mean`/`std`,
//! and a `503` from `/run` — is validated end-to-end.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn state() -> AppState {
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
        session_forker: None,
        usage_reader: None,
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
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        teams: None,
        incidents: None,
        team_audit: None,
        watchers: None,
        loops: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
    }
}

/// A complete `ZScore` spec as JSON: `window`/`cool_off` are integer seconds;
/// `detector`/`on_anomaly` are `kind`-tagged and `snake_case`.
fn zscore_spec(min_count: u64) -> serde_json::Value {
    serde_json::json!({
        "id": "orders",
        "kpi_query": "n/a",
        "window": 3600,
        "detector": { "kind": "z_score", "sigma_threshold": 3.0, "min_count": min_count },
        "cool_off": 0,
        "on_anomaly": { "kind": "notify", "channel": "ops" }
    })
}

/// 20 flat points (alternating 99/101) then a spike, as CSV.
fn spike_csv() -> String {
    let mut s = String::from("ts,value\n");
    for i in 0..20 {
        let v = if i % 2 == 0 { 99 } else { 101 };
        s.push_str(&format!("{i},{v}\n"));
    }
    s.push_str("20,5000\n");
    s
}

async fn post(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn run_returns_503_in_single_binary_build() {
    let app = router(state());
    // /run ignores the body and always 503s under DEC-033.
    let (status, body) = post(app, "/v1/anomaly/run", zscore_spec(10)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {body}");
    assert_eq!(body["code"], "service_unavailable");
}

#[tokio::test]
async fn test_back_tests_and_flags_spike() {
    let app = router(state());
    let req = serde_json::json!({
        "spec": zscore_spec(10),
        "csv": spike_csv(),
        "ts_col": "ts",
        "val_col": "value",
    });
    let (status, body) = post(app, "/v1/anomaly/test", req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let anomalies = body["anomalies"].as_array().expect("anomalies array");
    assert_eq!(anomalies.len(), 1, "exactly the spike should fire: {body}");
    let a = &anomalies[0];
    // The CLI table reads these exact keys — assert the contract.
    assert!(a["mean"].is_number(), "mean present: {a}");
    assert!(a["std"].is_number(), "std present: {a}");
    assert!(a["score"].as_f64().unwrap() > 3.0);
    assert!((a["value"].as_f64().unwrap() - 5000.0).abs() < f64::EPSILON);
    assert!(a["ts"].as_str().unwrap().starts_with("1970-01-01T00:00:20"));
    assert!(body["summary"].as_str().unwrap().contains("zscore"));
}

#[tokio::test]
async fn test_constant_series_returns_no_anomalies() {
    let app = router(state());
    let req = serde_json::json!({
        "spec": zscore_spec(5),
        "csv": "ts,value\n0,100\n1,100\n2,100\n3,100\n4,100\n5,100\n6,100\n",
        "ts_col": "ts",
        "val_col": "value",
    });
    let (status, body) = post(app, "/v1/anomaly/test", req).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["anomalies"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_rejects_missing_value_column() {
    let app = router(state());
    let req = serde_json::json!({
        "spec": zscore_spec(10),
        "csv": "ts,other\n0,1\n",
        "ts_col": "ts",
        "val_col": "value",
    });
    let (status, body) = post(app, "/v1/anomaly/test", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "bad_request");
    assert!(body["message"].as_str().unwrap().contains("value column"));
}

#[tokio::test]
async fn test_rejects_invalid_detector_alpha() {
    let app = router(state());
    let spec = serde_json::json!({
        "id": "x", "kpi_query": "n/a", "window": 3600,
        "detector": { "kind": "ewma", "alpha": 5.0, "sigma_threshold": 3.0, "min_count": 10 },
        "cool_off": 0,
        "on_anomaly": { "kind": "notify", "channel": "ops" }
    });
    let req = serde_json::json!({
        "spec": spec, "csv": "ts,value\n0,100\n", "ts_col": "ts", "val_col": "value",
    });
    let (status, body) = post(app, "/v1/anomaly/test", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body["message"].as_str().unwrap().contains("alpha"));
}
