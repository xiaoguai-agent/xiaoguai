//! Per-session turn lock regression tests (/loop L1 prerequisite,
//! LLD-LOOP-001 §3 gate: "concurrent-turn 409 regression test").
//!
//! Before this lock, turn serialisation was a CLIENT convention only —
//! `CancelRegistry::register` silently evicted the prior token, so two
//! concurrent `POST .../messages` on one session raced each other's
//! finalize/persist. Now the second request is refused with 409 while the
//! first turn (run + finalize) is still in flight, and succeeds once the
//! turn completes.

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use futures::stream;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::{ChatChunk, ChatRequest, ChatStream, FinishReason, LlmBackend, LlmError};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

/// Backend whose `chat_stream` parks until the test releases a permit —
/// holds a turn in flight for as long as the test needs.
#[derive(Debug)]
struct BlockingBackend {
    gate: Arc<Semaphore>,
}

#[async_trait]
impl LlmBackend for BlockingBackend {
    async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        let permit = self.gate.acquire().await.expect("gate closed");
        permit.forget();
        Ok(Box::pin(stream::iter(vec![
            Ok(ChatChunk {
                delta: "unblocked reply".into(),
                ..Default::default()
            }),
            Ok(ChatChunk {
                finish_reason: Some(FinishReason::Stop),
                done: true,
                ..Default::default()
            }),
        ])))
    }

    fn name(&self) -> &'static str {
        "blocking-mock"
    }
}

fn build_state(backend: Arc<dyn LlmBackend>) -> AppState {
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
        loops: None,
        decision_registry: Arc::new(xiaoguai_api::hotl::decision_registry::DecisionRegistry::new()),
    }
}

fn json_post(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_to_value(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.expect("read body");
    let s = String::from_utf8(bytes.to_vec()).expect("utf8");
    serde_json::from_str(&s).unwrap_or_else(|_| panic!("not valid JSON: {s}"))
}

async fn create_session(app: &axum::Router) -> String {
    let resp = app
        .clone()
        .oneshot(json_post(
            "/v1/sessions",
            &json!({"user_id": "usr_a", "model": "mock-model"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    body_to_value(resp.into_body()).await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn send_message(sid: &str, content: &str) -> Request<Body> {
    json_post(
        &format!("/v1/sessions/{sid}/messages"),
        &json!({ "content": content }),
    )
}

/// Wait until the per-session turn lock releases (the finalize task drops
/// the guard once output is persisted). Panics after ~5 s.
async fn wait_for_lock_release(state: &AppState, sid: &str) {
    for _ in 0..500 {
        if !state.cancels.is_active(sid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn lock for {sid} never released");
}

/// Drain an SSE body to completion under a hard deadline — a hung stream
/// must fail the test, never wedge the runner (CI runner-death war, #243).
async fn drain_sse(body: Body) {
    tokio::time::timeout(Duration::from_secs(10), to_bytes(body, 1024 * 1024))
        .await
        .expect("SSE stream did not close within 10s")
        .expect("read sse body");
}

#[tokio::test]
async fn concurrent_turn_on_same_session_is_409() {
    let gate = Arc::new(Semaphore::new(0));
    let state = build_state(Arc::new(BlockingBackend { gate: gate.clone() }));
    let app = router(state.clone());
    let sid = create_session(&app).await;

    // First turn: response head returns immediately, run parks on the gate.
    let first = app
        .clone()
        .oneshot(send_message(&sid, "first"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert!(state.cancels.is_active(&sid), "turn lock should be held");

    // Second turn on the SAME session while the first is in flight → 409.
    let second = app
        .clone()
        .oneshot(send_message(&sid, "second"))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let v = body_to_value(second.into_body()).await;
    assert_eq!(v["code"], "conflict");
    assert_eq!(
        v["message"],
        "conflict: a turn is already in flight for this session"
    );

    // A DIFFERENT session is not blocked by this session's turn.
    let other_sid = create_session(&app).await;
    gate.add_permits(8); // unblock present + future runs
    let other = app
        .clone()
        .oneshot(send_message(&other_sid, "other session"))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);

    // Drain the first response body (the SSE stream) to completion, then
    // wait for the finalize task to release the lock.
    drain_sse(first.into_body()).await;
    wait_for_lock_release(&state, &sid).await;

    // Persist-before-release: once the lock is free, the first turn's
    // assistant reply must already be in the session history — a follow-up
    // turn always sees the previous turn's messages (LLD-LOOP-001 §3).
    let history = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/sessions/{sid}/messages"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(history.status(), StatusCode::OK);
    let msgs = body_to_value(history.into_body()).await;
    // Domain `content` is a Vec<ContentBlock>; pull every text block.
    let contents: Vec<&str> = msgs
        .as_array()
        .expect("message list")
        .iter()
        .flat_map(|m| m["content"].as_array().into_iter().flatten())
        .filter_map(|block| block["text"].as_str())
        .collect();
    assert!(
        contents.contains(&"unblocked reply"),
        "first turn's reply must be persisted before the lock releases, got: {contents:?}"
    );

    // Sequential turn after completion succeeds again.
    let third = app
        .clone()
        .oneshot(send_message(&sid, "third"))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
}

#[tokio::test]
async fn cancel_works_while_turn_in_flight() {
    let gate = Arc::new(Semaphore::new(0));
    let state = build_state(Arc::new(BlockingBackend { gate: gate.clone() }));
    let app = router(state.clone());
    let sid = create_session(&app).await;

    let first = app
        .clone()
        .oneshot(send_message(&sid, "to be cancelled"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Cancel the in-flight turn — the registry entry is the turn lock, so
    // cancellation must find it.
    let resp = app
        .clone()
        .oneshot(json_post(&format!("/v1/sessions/{sid}/cancel"), &json!({})))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_to_value(resp.into_body()).await;
    assert_eq!(v["cancelled"], true);

    // Release the gate AFTER the cancel: if the agent task was already
    // parked inside the backend call, it can now return and observe the
    // cancellation; if it had not started yet, it exits at the loop-top
    // cancel check. Either way the run must finalise and release the lock
    // — without the permits a parked run would hang this drain forever
    // (review HIGH-1: never leave an unbounded wait in a test).
    gate.add_permits(8);
    drain_sse(first.into_body()).await;
    wait_for_lock_release(&state, &sid).await;
}
