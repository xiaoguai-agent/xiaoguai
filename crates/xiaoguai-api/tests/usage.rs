//! Integration coverage for v1.1.1: `GET /v1/usage`.
//!
//! The PG aggregation lives in `xiaoguai-core/src/usage_bridge.rs`;
//! here we drive the route handler with a `StaticUsageReader` to assert
//! the wire-shape, the 503 unwired path, the `InvalidRequest` path,
//! and that the `group_by` enum decodes from the query string.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::usage::{StaticUsageEntry, StaticUsageReader, UsageReader};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(usage_reader: Option<Arc<dyn UsageReader>>) -> AppState {
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
        authz: None,
        tenants: None,
        rate_limiter: None,
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
        usage_reader,
        session_forker: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
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
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn usage_503_when_reader_not_wired() {
    let app = router(build_state(None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn usage_default_group_by_is_day() {
    let d1 = Utc.with_ymd_and_hms(2026, 5, 20, 1, 0, 0).unwrap();
    let d2 = Utc.with_ymd_and_hms(2026, 5, 21, 2, 0, 0).unwrap();
    let reader = Arc::new(StaticUsageReader::with_entries(vec![
        StaticUsageEntry {
            ts: d1,
            tenant_id: "ten_a".into(),
            provider_id: "openai".into(),
            model: "gpt-4o".into(),
            input_tokens: 10,
            output_tokens: 5,
            cost_cents: None,
        },
        StaticUsageEntry {
            ts: d2,
            tenant_id: "ten_a".into(),
            provider_id: "openai".into(),
            model: "gpt-4o".into(),
            input_tokens: 20,
            output_tokens: 7,
            cost_cents: None,
        },
    ])) as Arc<dyn UsageReader>;
    let app = router(build_state(Some(reader)));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total_input_tokens"], 30);
    assert_eq!(body["total_output_tokens"], 12);
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["bucket"], "2026-05-20");
    assert_eq!(rows[1]["bucket"], "2026-05-21");
    // Cost rates deferred → top-level cost_cents is null.
    assert!(body["cost_cents"].is_null());
}

#[tokio::test]
async fn usage_group_by_provider_query_param() {
    let ts = Utc::now();
    let reader = Arc::new(StaticUsageReader::with_entries(vec![
        StaticUsageEntry {
            ts,
            tenant_id: "ten".into(),
            provider_id: "openai".into(),
            model: "gpt-4o".into(),
            input_tokens: 1,
            output_tokens: 1,
            cost_cents: None,
        },
        StaticUsageEntry {
            ts,
            tenant_id: "ten".into(),
            provider_id: "anthropic".into(),
            model: "claude".into(),
            input_tokens: 2,
            output_tokens: 2,
            cost_cents: None,
        },
    ])) as Arc<dyn UsageReader>;
    let app = router(build_state(Some(reader)));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage?group_by=provider")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let buckets: Vec<&str> = rows.iter().filter_map(|r| r["bucket"].as_str()).collect();
    assert!(buckets.contains(&"openai"));
    assert!(buckets.contains(&"anthropic"));
}

#[tokio::test]
async fn usage_tenant_filter_narrows_rows() {
    let ts = Utc::now();
    let reader = Arc::new(StaticUsageReader::with_entries(vec![
        StaticUsageEntry {
            ts,
            tenant_id: "ten_a".into(),
            provider_id: "openai".into(),
            model: "m".into(),
            input_tokens: 5,
            output_tokens: 1,
            cost_cents: None,
        },
        StaticUsageEntry {
            ts,
            tenant_id: "ten_b".into(),
            provider_id: "openai".into(),
            model: "m".into(),
            input_tokens: 100,
            output_tokens: 100,
            cost_cents: None,
        },
    ])) as Arc<dyn UsageReader>;
    let app = router(build_state(Some(reader)));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage?tenant_id=ten_a&group_by=model")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total_input_tokens"], 5);
    assert_eq!(body["total_output_tokens"], 1);
}

#[tokio::test]
async fn usage_400_when_since_after_until() {
    // The reader is wired; the handler should short-circuit before
    // invoking it.
    let reader = Arc::new(StaticUsageReader::with_entries(vec![])) as Arc<dyn UsageReader>;
    let app = router(build_state(Some(reader)));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage?since=2026-05-22T00:00:00Z&until=2026-05-20T00:00:00Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn usage_invalid_group_by_returns_400() {
    let reader = Arc::new(StaticUsageReader::with_entries(vec![])) as Arc<dyn UsageReader>;
    let app = router(build_state(Some(reader)));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage?group_by=hour")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // axum's Query extractor rejects unknown enum variants as 400.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
