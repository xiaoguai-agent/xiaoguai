//! End-to-end coverage for `POST /v1/im/dingtalk/webhook` — verifies
//! the signed-message routing, 401 on missing signature, and 400 on a
//! signed-but-malformed payload. Mirrors `feishu_route.rs`.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::Sha256;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{AppState, CancelRegistry};
use xiaoguai_im_dingtalk::DingTalkProvider;
use xiaoguai_im_gateway::{mount_dingtalk, ImProvider, OutgoingReply};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

mod common;
use common::{InMemoryMessageRepo, InMemorySessionRepo};

const SECRET: &str = "dingtalk-app-secret";
type HmacSha256 = Hmac<Sha256>;

fn sign(ts: &str) -> String {
    let string_to_sign = format!("{ts}\n{SECRET}");
    let mut mac = HmacSha256::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(string_to_sign.as_bytes());
    B64_STANDARD.encode(mac.finalize().into_bytes())
}

fn build_app(sink: Arc<Mutex<Vec<OutgoingReply>>>) -> axum::Router {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let state = AppState {
        sessions,
        messages,
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
    };
    let provider: Arc<dyn ImProvider> =
        Arc::new(DingTalkProvider::with_recording_sink(SECRET, sink));
    mount_dingtalk(state, provider)
}

fn signed_request(body: &str) -> Request<Body> {
    let ts = "1716355200000";
    let sig = sign(ts);
    Request::builder()
        .method(Method::POST)
        .uri("/v1/im/dingtalk/webhook")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Dingtalk-Timestamp", ts)
        .header("X-Dingtalk-Sign", sig)
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn message_event_is_accepted_and_reply_dispatched() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink.clone());
    let body = json!({
        "msgtype": "text",
        "text": {"content": "hi"},
        "conversationId": "cid_x",
        "conversationType": "1",
        "senderStaffId": "user_a",
        "senderCorpId": "corp_x",
        "msgId": "m1"
    })
    .to_string();
    let resp = app.oneshot(signed_request(&body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["status"], "accepted");

    // Wait for the background spawn to deliver its reply.
    for _ in 0..20 {
        if !sink.lock().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let buf = sink.lock();
    assert_eq!(buf.len(), 1, "exactly one reply should have been recorded");
    assert!(buf[0].conversation_id.starts_with("dingtalk:single:"));
    assert_eq!(buf[0].text, "noop");
}

#[tokio::test]
async fn missing_signature_yields_401() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/im/dingtalk/webhook")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_signed_body_yields_400() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let resp = app.oneshot(signed_request("not-json")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn group_chat_routes_separately() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink.clone());
    let body = json!({
        "msgtype": "text",
        "text": {"content": "hi group"},
        "conversationId": "cid_grp",
        "conversationType": "2",
        "senderStaffId": "user_b",
        "senderCorpId": "corp_x",
        "msgId": "m2"
    })
    .to_string();
    let resp = app.oneshot(signed_request(&body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    for _ in 0..20 {
        if !sink.lock().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let buf = sink.lock();
    assert_eq!(buf.len(), 1);
    assert!(buf[0].conversation_id.starts_with("dingtalk:group:"));
}
