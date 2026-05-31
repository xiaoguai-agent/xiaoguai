//! v1.1.2 — `POST /v1/sessions/:id/fork` route coverage.
//!
//! Uses the in-memory `InMemorySessionRepo` + a small `StaticForker`
//! shim so we exercise the handler → forker → response wire path
//! without spinning up Postgres. The end-to-end "fork copies the
//! prefix" semantics is covered by `sessions_bridge.rs` unit tests
//! and (when env permits) the Pg integration suite.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::sessions_ext::{SessionForkError, SessionForker};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_storage::repositories::SessionRepository;
use xiaoguai_types::{Session, SessionId, SessionStatus, TenantId, UserId};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

type ForkCall = (String, String, String, Option<String>);

struct RecordingForker {
    calls: Mutex<Vec<ForkCall>>,
    result: Mutex<Option<Result<Session, SessionForkError>>>,
}

impl RecordingForker {
    fn with_result(result: Result<Session, SessionForkError>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(result)),
        })
    }
}

#[async_trait]
impl SessionForker for RecordingForker {
    async fn fork(
        &self,
        tenant: &str,
        parent_id: &str,
        from_message_id: &str,
        title: Option<String>,
    ) -> Result<Session, SessionForkError> {
        self.calls.lock().push((
            tenant.to_string(),
            parent_id.to_string(),
            from_message_id.to_string(),
            title.clone(),
        ));
        // Consume the held result. Each test sets exactly one expected
        // outcome; a second call should fail loudly so we don't
        // accidentally double-fire.
        self.result
            .lock()
            .take()
            .unwrap_or_else(|| Err(SessionForkError::Repository("already consumed".into())))
    }
}

fn build_state(forker: Option<Arc<dyn SessionForker>>) -> (AppState, Arc<InMemorySessionRepo>) {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let state = AppState {
        sessions: sessions.clone(),
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
        session_forker: forker,
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
    (state, sessions)
}

async fn seed_parent(sessions: &InMemorySessionRepo, id: &str, tenant: &str) {
    let now = Utc::now();
    let s = Session {
        id: SessionId::from(id.to_string()),
        tenant_id: TenantId::from(tenant.to_string()),
        user_id: UserId::from("u".to_string()),
        title: Some("p".into()),
        created_at: now,
        updated_at: now,
        model: "m".into(),
        status: SessionStatus::Active,
        parent_session_id: None,
        forked_from_message_id: None,
    };
    sessions.create(None, &s).await.unwrap();
}

fn forked_session(parent_id: &str, tenant: &str, from_message_id: &str) -> Session {
    let now = Utc::now();
    Session {
        id: SessionId::new(),
        tenant_id: TenantId::from(tenant.to_string()),
        user_id: UserId::from("u".to_string()),
        title: Some("Fork: p".into()),
        created_at: now,
        updated_at: now,
        model: "m".into(),
        status: SessionStatus::Active,
        parent_session_id: Some(SessionId::from(parent_id.to_string())),
        forked_from_message_id: Some(xiaoguai_types::MessageId::from(from_message_id.to_string())),
    }
}

fn post(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_value(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn fork_returns_503_when_forker_not_wired() {
    let (state, sessions) = build_state(None);
    let _ = sessions; // sessions still need to exist; parent presence here doesn't matter
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/sess_x/fork",
            &json!({"from_message_id": "m1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn fork_returns_404_when_parent_missing() {
    let forker = RecordingForker::with_result(Ok(forked_session("p", "t", "m1")));
    let (state, _) = build_state(Some(forker as Arc<dyn SessionForker>));
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/sess_missing/fork",
            &json!({"from_message_id": "m1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fork_returns_400_when_message_id_blank() {
    let forker = RecordingForker::with_result(Ok(forked_session("p", "t", "m1")));
    let (state, sessions) = build_state(Some(forker as Arc<dyn SessionForker>));
    seed_parent(&sessions, "p1", "t").await;
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/p1/fork",
            &json!({"from_message_id": "   "}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn fork_happy_path_returns_201_with_child_session() {
    let child = forked_session("p1", "t", "m1");
    let child_id = child.id.as_str().to_string();
    let forker = RecordingForker::with_result(Ok(child));
    let forker_trait: Arc<dyn SessionForker> = forker.clone();
    let (state, sessions) = build_state(Some(forker_trait));
    seed_parent(&sessions, "p1", "t").await;
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/p1/fork",
            &json!({"from_message_id": "m1", "title": "explore"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_value(resp.into_body()).await;
    assert_eq!(v["id"], child_id);
    assert_eq!(v["parent_session_id"], "p1");
    assert_eq!(v["forked_from_message_id"], "m1");

    // Recorded a single call with the expected args. Tenant was lifted
    // from the parent session row (no auth header → no claims).
    let calls = forker.calls.lock();
    assert_eq!(calls.len(), 1);
    let (tenant, parent_id, msg_id, title) = &calls[0];
    assert_eq!(tenant, "t");
    assert_eq!(parent_id, "p1");
    assert_eq!(msg_id, "m1");
    assert_eq!(title.as_deref(), Some("explore"));
}

#[tokio::test]
async fn fork_maps_message_not_found_to_404() {
    let forker = RecordingForker::with_result(Err(SessionForkError::MessageNotFound));
    let (state, sessions) = build_state(Some(forker as Arc<dyn SessionForker>));
    seed_parent(&sessions, "p1", "t").await;
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/p1/fork",
            &json!({"from_message_id": "nope"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fork_maps_parent_not_forkable_to_409() {
    let forker =
        RecordingForker::with_result(Err(SessionForkError::ParentNotForkable("Archived".into())));
    let (state, sessions) = build_state(Some(forker as Arc<dyn SessionForker>));
    seed_parent(&sessions, "p1", "t").await;
    let app = router(state);
    let resp = app
        .oneshot(post(
            "/v1/sessions/p1/fork",
            &json!({"from_message_id": "m1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
