//! End-to-end coverage for `POST /v1/im/wecom/webhook` — verifies the
//! signed-message routing, 401 on missing signature, and 400 on a
//! signed-but-malformed payload. Mirrors `feishu_route.rs` /
//! `dingtalk_route.rs`.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use parking_lot::Mutex;
use serde_json::Value;
use sha1::{Digest, Sha1};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{AppState, CancelRegistry};
use xiaoguai_im_gateway::{mount_wecom, ImProvider, OutgoingReply};
use xiaoguai_im_wecom::WeComProvider;
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

mod common;
use common::{InMemoryMessageRepo, InMemorySessionRepo};

const TOKEN: &str = "wecom-callback-token";

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn sign(ts: &str, nonce: &str, body_text: &str) -> String {
    let mut parts = [TOKEN, ts, nonce, body_text];
    parts.sort_unstable();
    let mut hasher = Sha1::new();
    hasher.update(parts.concat().as_bytes());
    hex_lower(&hasher.finalize())
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
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: None,
    };
    let provider: Arc<dyn ImProvider> = Arc::new(WeComProvider::with_recording_sink(TOKEN, sink));
    mount_wecom(state, provider)
}

fn signed_request(body: &str) -> Request<Body> {
    let ts = "1716355200";
    let nonce = "nonce_x";
    let sig = sign(ts, nonce, body);
    Request::builder()
        .method(Method::POST)
        .uri("/v1/im/wecom/webhook")
        .header(header::CONTENT_TYPE, "application/xml")
        .header("X-WeCom-Timestamp", ts)
        .header("X-WeCom-Nonce", nonce)
        .header("X-WeCom-Msg-Signature", sig)
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
    let body = r"<xml>
        <ToUserName>corp_x</ToUserName>
        <FromUserName>user_a</FromUserName>
        <CreateTime>1716355200</CreateTime>
        <MsgType>text</MsgType>
        <Content>hi bot</Content>
        <MsgId>m1</MsgId>
        <AgentID>1000002</AgentID>
    </xml>";
    let resp = app.oneshot(signed_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["status"], "accepted");

    for _ in 0..20 {
        if !sink.lock().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let buf = sink.lock();
    assert_eq!(buf.len(), 1, "exactly one reply should have been recorded");
    assert!(buf[0].conversation_id.starts_with("wecom:"));
    assert_eq!(buf[0].text, "noop");
}

#[tokio::test]
async fn missing_signature_yields_401() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/im/wecom/webhook")
        .body(Body::from("<xml></xml>"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_signed_body_yields_400() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let resp = app.oneshot(signed_request("not-xml-at-all")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn encrypted_payload_is_rejected_as_400() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let body = r"<xml><Encrypt>blob</Encrypt><ToUserName>corp_x</ToUserName></xml>";
    let resp = app.oneshot(signed_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
