//! End-to-end coverage for the v0.5.5 axum surface.
//!
//! These tests drive the router via `tower::ServiceExt::oneshot`, so they
//! exercise the real handlers + middleware stack without binding a port.
//! SSE responses are assembled by reading the response body to bytes.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend, ToolCallSpec};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(steps: Vec<ScriptStep>) -> (AppState, Arc<InMemoryMessageRepo>) {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(steps));
    let toolbox = Arc::new(Toolbox::new());
    let state = AppState {
        sessions,
        messages: messages.clone(),
        backend,
        toolbox,
        agent_defaults: AgentConfig::new("mock-model"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        authz: None,
        tenants: None,
        rate_limiter: None,
        audit: None,
        audit_verifier: None,
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
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
    };
    (state, messages)
}

async fn body_to_string(body: Body) -> String {
    let bytes = to_bytes(body, 1024 * 1024).await.expect("read body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn body_to_value(body: Body) -> Value {
    let s = body_to_string(body).await;
    serde_json::from_str(&s).unwrap_or_else(|_| panic!("not valid JSON: {s}"))
}

fn json_post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn healthz_returns_ok() {
    let (state, _) = build_state(vec![ScriptStep::text("noop")]);
    let app = router(state);
    let resp = app.oneshot(get("/healthz")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_to_string(resp.into_body()).await, "ok");
}

#[tokio::test]
async fn create_session_returns_201_and_body() {
    let (state, _) = build_state(vec![ScriptStep::text("noop")]);
    let app = router(state);
    let body = json!({
        "user_id": "usr_a",
        "tenant_id": "ten_a",
        "model": "mock-model",
        "title": "demo"
    });
    let resp = app.oneshot(json_post("/v1/sessions", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_to_value(resp.into_body()).await;
    assert_eq!(v["user_id"], "usr_a");
    assert_eq!(v["model"], "mock-model");
    assert!(v["id"].as_str().unwrap().starts_with("sess_"));
}

#[tokio::test]
async fn create_session_rejects_empty_fields() {
    let (state, _) = build_state(vec![ScriptStep::text("noop")]);
    let app = router(state);
    let body = json!({"user_id":"","tenant_id":"","model":""});
    let resp = app.oneshot(json_post("/v1/sessions", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_to_value(resp.into_body()).await;
    assert_eq!(v["code"], "bad_request");
}

#[tokio::test]
async fn get_session_returns_404_for_unknown() {
    let (state, _) = build_state(vec![ScriptStep::text("noop")]);
    let app = router(state);
    let resp = app.oneshot(get("/v1/sessions/sess_missing")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn create_and_get_session_id(app: axum::Router) -> (axum::Router, String) {
    let body = json!({
        "user_id": "usr_a",
        "tenant_id": "ten_a",
        "model": "mock-model"
    });
    let resp = app
        .clone()
        .oneshot(json_post("/v1/sessions", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_to_value(resp.into_body()).await;
    let id = v["id"].as_str().unwrap().to_string();
    (app, id)
}

#[tokio::test]
async fn send_message_streams_sse_and_persists_messages() {
    let (state, messages) = build_state(vec![ScriptStep::text("hello back")]);
    let app = router(state);
    let (app, sid) = create_and_get_session_id(app).await;

    let resp = app
        .oneshot(json_post(
            &format!("/v1/sessions/{sid}/messages"),
            json!({"content": "hi"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ctype.starts_with("text/event-stream"), "got {ctype}");

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    // SSE format: event: <name>\ndata: <json>\n\n
    assert!(text.contains("event: text_delta"), "stream:\n{text}");
    assert!(text.contains("hello back"), "stream:\n{text}");
    assert!(text.contains("event: done"), "stream:\n{text}");

    // Give the finalize task a chance to land its writes.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let stored = messages.snapshot(&sid);
    // user + assistant
    assert_eq!(stored.len(), 2, "stored = {stored:?}");
    assert_eq!(stored[0].role, xiaoguai_types::MessageRole::User);
    assert_eq!(stored[1].role, xiaoguai_types::MessageRole::Assistant);
}

#[tokio::test]
async fn send_message_with_tool_call_persists_tool_blocks() {
    let (state, messages) = build_state(vec![
        ScriptStep::tool_calls(vec![ToolCallSpec {
            id: "c1".into(),
            name: "ghost".into(),
            arguments_json: r#"{"q":"x"}"#.into(),
        }]),
        ScriptStep::text("fallback"),
    ]);
    let app = router(state);
    let (app, sid) = create_and_get_session_id(app).await;

    let resp = app
        .oneshot(json_post(
            &format!("/v1/sessions/{sid}/messages"),
            json!({"content": "hi"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let stored = messages.snapshot(&sid);
    // user + assistant(tool_calls) + tool(result for unknown tool) + assistant(text)
    assert_eq!(stored.len(), 4, "stored = {stored:?}");
    assert_eq!(stored[1].role, xiaoguai_types::MessageRole::Assistant);
    assert_eq!(stored[2].role, xiaoguai_types::MessageRole::Tool);
    assert_eq!(stored[3].role, xiaoguai_types::MessageRole::Assistant);
}

#[tokio::test]
async fn list_messages_returns_persisted_history_after_send() {
    let (state, _) = build_state(vec![ScriptStep::text("ok")]);
    let app = router(state);
    let (app, sid) = create_and_get_session_id(app).await;

    let send = app
        .clone()
        .oneshot(json_post(
            &format!("/v1/sessions/{sid}/messages"),
            json!({"content": "hi"}),
        ))
        .await
        .unwrap();
    let _ = send.into_body().collect().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let resp = app
        .oneshot(get(&format!("/v1/sessions/{sid}/messages")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_to_value(resp.into_body()).await;
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[tokio::test]
async fn list_messages_404_for_unknown_session() {
    let (state, _) = build_state(vec![ScriptStep::text("ok")]);
    let app = router(state);
    let resp = app
        .oneshot(get("/v1/sessions/sess_missing/messages"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_message_empty_content_is_rejected() {
    let (state, _) = build_state(vec![ScriptStep::text("ok")]);
    let app = router(state);
    let (app, sid) = create_and_get_session_id(app).await;

    let resp = app
        .oneshot(json_post(
            &format!("/v1/sessions/{sid}/messages"),
            json!({"content": "   "}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_returns_false_when_no_active_run() {
    let (state, _) = build_state(vec![ScriptStep::text("ok")]);
    let app = router(state);
    let (app, sid) = create_and_get_session_id(app).await;

    let resp = app
        .oneshot(json_post(&format!("/v1/sessions/{sid}/cancel"), json!({})))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_to_value(resp.into_body()).await;
    assert_eq!(v["cancelled"], false);
}
