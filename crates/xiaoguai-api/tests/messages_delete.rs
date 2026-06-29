//! `DELETE /v1/sessions/{session_id}/messages/{message_id}` route coverage.
//!
//! Uses the in-memory `InMemorySessionRepo` + `InMemoryMessageRepo` (same
//! harness as `fork.rs`) so we exercise the handler → repo → HTTP-status wire
//! path without a database. The repo's session-scoped delete semantics
//! (`NotFound` on a missing/cross-session id) are covered separately in
//! `xiaoguai-storage`'s `message_repo.rs`.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_storage::repositories::{MessageRepository, SessionRepository};
use xiaoguai_types::{
    ContentBlock, Message, MessageId, MessageRole, Session, SessionId, SessionStatus, UserId,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state() -> (AppState, Arc<InMemorySessionRepo>, Arc<InMemoryMessageRepo>) {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let state = AppState {
        sessions: sessions.clone(),
        messages: messages.clone(),
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
    };
    (state, sessions, messages)
}

async fn seed_session(sessions: &InMemorySessionRepo, id: &str) {
    let now = Utc::now();
    let s = Session {
        id: SessionId::from(id.to_string()),
        user_id: UserId::from("u".to_string()),
        title: None,
        created_at: now,
        updated_at: now,
        model: "m".into(),
        status: SessionStatus::Active,
        parent_session_id: None,
        forked_from_message_id: None,
        working_dir: None,
    };
    sessions.create(&s).await.unwrap();
}

async fn seed_message(messages: &InMemoryMessageRepo, session_id: &str, message_id: &str) {
    let msg = Message {
        id: MessageId::from(message_id.to_string()),
        session_id: SessionId::from(session_id.to_string()),
        role: MessageRole::User,
        content: vec![ContentBlock::Text {
            text: "hi".to_string(),
        }],
        created_at: Utc::now(),
    };
    messages.append(&msg).await.unwrap();
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn delete_existing_message_returns_204() {
    let (state, sessions, messages) = build_state();
    seed_session(&sessions, "s1").await;
    seed_message(&messages, "s1", "m1").await;
    let app = router(state);

    let resp = app
        .oneshot(delete("/v1/sessions/s1/messages/m1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The message is gone — the session now has zero messages.
    assert_eq!(messages.snapshot("s1").len(), 0);
}

#[tokio::test]
async fn delete_missing_message_returns_404() {
    let (state, sessions, _messages) = build_state();
    seed_session(&sessions, "s1").await;
    let app = router(state);

    let resp = app
        .oneshot(delete("/v1/sessions/s1/messages/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_message_with_wrong_session_returns_404() {
    let (state, sessions, messages) = build_state();
    seed_session(&sessions, "s1").await;
    seed_session(&sessions, "s2").await;
    seed_message(&messages, "s1", "m1").await;
    let app = router(state);

    // The message lives in s1; deleting it under s2 must 404 and leave it.
    let resp = app
        .oneshot(delete("/v1/sessions/s2/messages/m1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(messages.snapshot("s1").len(), 1);
}
