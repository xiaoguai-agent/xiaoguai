//! Integration tests for the v1.2.4 outcome telemetry endpoints.
//!
//! Tests cover:
//!   - `POST /v1/outcomes` — record attribution
//!   - `GET /v1/outcomes/summary` — ROI summary cards
//!   - `GET /v1/outcomes/timeseries` — bar chart data
//!   - 503 responses when the backends are unwired
//!   - input validation at the route level

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::outcomes::{InMemoryOutcomesBackend, OutcomeWriter, OutcomesReader};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_state() -> AppState {
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock-model"),
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
        watchers: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
    }
}

fn state_with_backend(backend: Arc<InMemoryOutcomesBackend>) -> AppState {
    let mut s = minimal_state();
    s.outcome_writer = Some(backend.clone() as Arc<dyn OutcomeWriter>);
    s.outcomes_reader = Some(backend as Arc<dyn OutcomesReader>);
    s
}

fn fresh_backend() -> Arc<InMemoryOutcomesBackend> {
    Arc::new(InMemoryOutcomesBackend::new())
}

async fn body_json(body: Body) -> Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// POST /v1/outcomes — record attribution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn record_outcome_returns_201() {
    let backend = fresh_backend();
    let app = router(state_with_backend(backend.clone()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/outcomes")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "tenant_id": "tenant_a",
                        "session_id": "sess_1",
                        "agent_name": "sales-bot",
                        "kind": "revenue_usd",
                        "value": 500.0,
                        "unit": "usd",
                        "description": "Closed enterprise deal",
                        "metadata": {"deal_id": "D42"}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let snap = backend.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].kind, "revenue_usd");
}

#[tokio::test]
async fn record_outcome_rejects_negative_value() {
    let app = router(state_with_backend(fresh_backend()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/outcomes")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "tenant_id": "t1",
                        "agent_name": "bot",
                        "kind": "revenue_usd",
                        "value": -10.0
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn record_outcome_rejects_empty_kind() {
    let app = router(state_with_backend(fresh_backend()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/outcomes")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "tenant_id": "t1",
                        "agent_name": "bot",
                        "kind": "",
                        "value": 10.0
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn record_outcome_503_when_unwired() {
    let app = router(minimal_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/outcomes")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "tenant_id": "t1",
                        "agent_name": "bot",
                        "kind": "revenue_usd",
                        "value": 100.0
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/summary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn summary_returns_by_kind_map() {
    use xiaoguai_api::outcomes::RecordOutcomeRequest;
    let backend = fresh_backend();
    // Pre-populate two kinds.
    let state = state_with_backend(backend.clone());
    // Write directly via the backend to avoid another HTTP round-trip.
    backend
        .record(RecordOutcomeRequest {
            session_id: None,
            agent_name: "bot".into(),
            kind: "revenue_usd".into(),
            value: 100.0,
            unit: None,
            description: None,
            metadata: json!({}),
        })
        .await
        .unwrap();
    backend
        .record(RecordOutcomeRequest {
            session_id: None,
            agent_name: "bot".into(),
            kind: "hours_saved".into(),
            value: 8.0,
            unit: Some("hours".into()),
            description: None,
            metadata: json!({}),
        })
        .await
        .unwrap();

    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/summary?tenant_id=ten&range=30d")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["range"], "30d");
    let rev = &body["summary"]["by_kind"]["revenue_usd"];
    assert!((rev["sum"].as_f64().unwrap() - 100.0).abs() < f64::EPSILON);
    assert_eq!(rev["count"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn summary_503_when_unwired() {
    let app = router(minimal_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/summary?tenant_id=ten")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn summary_defaults_empty_tenant_to_owner() {
    // DEC-033 single-owner: an empty tenant_id defaults to the owner tenant
    // and the request succeeds rather than 400ing.
    let app = router(state_with_backend(fresh_backend()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/summary?tenant_id=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn summary_rejects_unknown_range() {
    let app = router(state_with_backend(fresh_backend()));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/summary?tenant_id=ten&range=99y")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// GET /v1/outcomes/timeseries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timeseries_returns_day_buckets() {
    use xiaoguai_api::outcomes::RecordOutcomeRequest;
    let backend = fresh_backend();
    for _ in 0..3 {
        backend
            .record(RecordOutcomeRequest {
                session_id: None,
                agent_name: "bot".into(),
                kind: "deals_closed".into(),
                value: 1.0,
                unit: Some("count".into()),
                description: None,
                metadata: json!({}),
            })
            .await
            .unwrap();
    }

    let app = router(state_with_backend(backend));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/timeseries?tenant_id=ten&range=7d&kind=deals_closed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let days = body["days"].as_array().unwrap();
    assert_eq!(days.len(), 1);
    assert!((days[0]["sum"].as_f64().unwrap() - 3.0).abs() < f64::EPSILON);
    assert_eq!(days[0]["count"].as_u64().unwrap(), 3);
}

#[tokio::test]
async fn timeseries_503_when_unwired() {
    let app = router(minimal_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/outcomes/timeseries?tenant_id=ten")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
