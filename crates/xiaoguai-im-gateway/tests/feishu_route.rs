//! End-to-end coverage for `POST /v1/im/feishu/webhook` — verifies the
//! signed-challenge handshake, signed-message routing, and 401 on missing
//! signature.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{AppState, CancelRegistry};
use xiaoguai_im_feishu::FeishuProvider;
use xiaoguai_im_gateway::{mount_feishu, ImProvider, OutgoingReply};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

mod common;
use common::{InMemoryMessageRepo, InMemorySessionRepo};

const KEY: &str = "test-encrypt-key";

fn sign(body: &str, ts: &str, nonce: &str) -> String {
    let mut h = Sha256::new();
    h.update(ts.as_bytes());
    h.update(nonce.as_bytes());
    h.update(KEY.as_bytes());
    h.update(body.as_bytes());
    hex::encode(h.finalize())
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
    };
    let provider: Arc<dyn ImProvider> = Arc::new(FeishuProvider::with_recording_sink(KEY, sink));
    mount_feishu(state, provider)
}

fn signed_request(body: &str) -> Request<Body> {
    let ts = "1716355200";
    let nonce = "abc123";
    let sig = sign(body, ts, nonce);
    Request::builder()
        .method(Method::POST)
        .uri("/v1/im/feishu/webhook")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Lark-Request-Timestamp", ts)
        .header("X-Lark-Request-Nonce", nonce)
        .header("X-Lark-Signature", sig)
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn challenge_round_trips() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let body = r#"{"challenge":"hello"}"#;
    let resp = app.oneshot(signed_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["challenge"], "hello");
}

#[tokio::test]
async fn message_event_is_accepted_and_reply_dispatched() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink.clone());
    let body = json!({
        "header": {"event_id": "evt-1", "tenant_key": "ten_x"},
        "event": {
            "sender": {"sender_id": {"open_id": "ou_alice"}, "tenant_key": "ten_x"},
            "message": {"chat_id": "oc_chat", "content": "{\"text\":\"hi\"}"}
        }
    })
    .to_string();
    let resp = app.oneshot(signed_request(&body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["status"], "accepted");

    // The ReactAgent loop runs against the scripted MockBackend; allow
    // a short grace period for the background spawn to deliver its
    // reply into the recording sink.
    for _ in 0..20 {
        if !sink.lock().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let buf = sink.lock();
    assert_eq!(buf.len(), 1, "exactly one reply should have been recorded");
    assert_eq!(buf[0].conversation_id, "oc_chat");
    // v0.7.1: the reply text is the MockBackend's scripted output, not
    // an echo of the user's input. The script returns "noop".
    assert_eq!(buf[0].text, "noop");
}

#[tokio::test]
async fn missing_signature_yields_401() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/im/feishu/webhook")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_signed_body_yields_400() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let app = build_app(sink);
    // Properly signed but garbage payload.
    let resp = app.oneshot(signed_request("not-json")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// v0.6.5: the IM gateway must propagate the per-conversation tenant
/// resolved by `ImHistoryStore::resolve_tenant` onto the agent build,
/// so v0.6.4's per-tenant `LlmRouter` defaults apply to IM traffic too.
/// Driven through `run_agent_and_reply` directly with a recording history
/// store that returns a fixed tenant.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agent_inherits_tenant_resolved_by_history_store() {
    use async_trait::async_trait;
    use parking_lot::Mutex as PlMutex;
    use std::sync::Arc;
    use xiaoguai_im_feishu::FeishuProvider;
    use xiaoguai_im_gateway::{
        run_agent_and_reply, ConversationIdent, GatewayState, HistoryError, ImHistoryStore,
        ImProvider, IncomingMessage,
    };
    use xiaoguai_llm::Message as LlmMessage;

    /// Records the request's `tenant_id` so the test can assert routing
    /// flowed through `AgentConfig.tenant_id` → `ChatRequest.tenant_id`.
    struct CapturingBackend {
        captured_tenant: Arc<PlMutex<Option<String>>>,
    }

    #[async_trait]
    impl LlmBackend for CapturingBackend {
        async fn chat_stream(
            &self,
            req: xiaoguai_llm::ChatRequest,
        ) -> Result<xiaoguai_llm::ChatStream, xiaoguai_llm::LlmError> {
            *self.captured_tenant.lock() = req.tenant_id.clone();
            let chunks = vec![
                Ok(xiaoguai_llm::ChatChunk {
                    delta: "ok".into(),
                    ..Default::default()
                }),
                Ok(xiaoguai_llm::ChatChunk {
                    delta: String::new(),
                    tool_calls: vec![],
                    finish_reason: Some(xiaoguai_llm::FinishReason::Stop),
                    done: true,
                    reasoning_delta: None,
                }),
            ];
            Ok(Box::pin(futures::stream::iter(chunks)))
        }
        fn name(&self) -> &'static str {
            "capturing"
        }
    }

    /// Tiny stub store that always reports "`ten_fixed`".
    struct FixedTenantStore {
        inner: PlMutex<Vec<LlmMessage>>,
    }

    #[async_trait]
    impl ImHistoryStore for FixedTenantStore {
        async fn snapshot(
            &self,
            _ident: &ConversationIdent,
        ) -> Result<Vec<LlmMessage>, HistoryError> {
            Ok(self.inner.lock().clone())
        }
        async fn extend(
            &self,
            _ident: &ConversationIdent,
            msgs: Vec<LlmMessage>,
        ) -> Result<(), HistoryError> {
            self.inner.lock().extend(msgs);
            Ok(())
        }
        async fn resolve_tenant(
            &self,
            _ident: &ConversationIdent,
        ) -> Result<Option<String>, HistoryError> {
            Ok(Some("ten_fixed".into()))
        }
    }

    let captured = Arc::new(PlMutex::new(None));
    let backend: Arc<dyn LlmBackend> = Arc::new(CapturingBackend {
        captured_tenant: captured.clone(),
    });
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let app_state = AppState {
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
    };
    let sink = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ImProvider> = Arc::new(FeishuProvider::with_recording_sink(KEY, sink));
    let history: Arc<dyn ImHistoryStore> = Arc::new(FixedTenantStore {
        inner: PlMutex::new(Vec::new()),
    });
    let state = GatewayState {
        app: app_state,
        provider,
        history,
    };
    let msg = IncomingMessage {
        provider: "feishu".into(),
        user_external_id: "ou".into(),
        tenant_external_id: "tk".into(),
        conversation_id: "oc".into(),
        text: "hi".into(),
        event_id: "evt".into(),
    };
    run_agent_and_reply(state, msg).await.expect("ok");
    assert_eq!(captured.lock().as_deref(), Some("ten_fixed"));
}

/// v0.7.2/v0.7.3: prove that subsequent webhooks for the *same*
/// `conversation_id` see the accumulated history, while a different
/// `conversation_id` does not. Driven through `run_agent_and_reply`
/// directly so the test does not race the background spawn. Uses the
/// in-memory `ConversationHistory` cast to the `ImHistoryStore` trait so
/// the production code path (trait dispatch) is exercised.
#[tokio::test]
async fn conversation_history_accumulates_per_chat() {
    use xiaoguai_im_feishu::FeishuProvider;
    use xiaoguai_im_gateway::{
        run_agent_and_reply, ConversationHistory, GatewayState, ImHistoryStore, ImProvider,
        IncomingMessage,
    };

    // Script three distinct assistant outputs so each turn is
    // distinguishable.
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::text("first"),
        ScriptStep::text("second"),
        ScriptStep::text("third"),
    ]));
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let app_state = AppState {
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
    };
    let sink = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn ImProvider> = Arc::new(FeishuProvider::with_recording_sink(KEY, sink));
    let history_concrete = Arc::new(ConversationHistory::new(10));
    let history: Arc<dyn ImHistoryStore> = history_concrete.clone();
    let state = GatewayState {
        app: app_state,
        provider,
        history,
    };

    let mk_msg = |chat: &str, text: &str| IncomingMessage {
        provider: "feishu".into(),
        user_external_id: "ou_alice".into(),
        tenant_external_id: "ten_x".into(),
        conversation_id: chat.into(),
        text: text.into(),
        event_id: "evt".into(),
    };

    run_agent_and_reply(state.clone(), mk_msg("oc_a", "hello"))
        .await
        .expect("turn 1");
    run_agent_and_reply(state.clone(), mk_msg("oc_a", "and again"))
        .await
        .expect("turn 2");
    run_agent_and_reply(state.clone(), mk_msg("oc_b", "different chat"))
        .await
        .expect("turn 3");

    // oc_a saw two user+assistant turns = 4 messages.
    let a = history_concrete.snapshot("oc_a");
    assert_eq!(a.len(), 4, "oc_a should have accumulated 2 turns");
    assert_eq!(a[0].content, "hello");
    assert_eq!(a[1].content, "first");
    assert_eq!(a[2].content, "and again");
    assert_eq!(a[3].content, "second");
    // oc_b is isolated — only one turn.
    let b = history_concrete.snapshot("oc_b");
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].content, "different chat");
    assert_eq!(b[1].content, "third");
}
