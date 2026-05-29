//! End-to-end coverage for the v0.5.6 `xiaoguai remote ...` path.
//!
//! Spawns `xiaoguai-api::serve_with_state` on `127.0.0.1:0` with an
//! in-memory repo backing store + scripted `MockBackend`, then drives the
//! same `RemoteClient` the binary uses. This is the production path minus
//! the PG side of the `AppState`.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{serve_with_state, AppState, CancelRegistry};
use xiaoguai_cli::commands::remote::{CreateSessionRequest, RemoteClient};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

mod common;
use common::{InMemoryMessageRepo, InMemorySessionRepo};

async fn spawn_server(steps: Vec<ScriptStep>) -> String {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(steps));
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
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
    };
    let (local, fut) = serve_with_state("127.0.0.1:0".parse().unwrap(), state)
        .await
        .expect("bind");
    tokio::spawn(fut);
    // Give the listener a tick to actually accept.
    tokio::time::sleep(Duration::from_millis(20)).await;
    format!("http://{local}")
}

#[tokio::test]
async fn healthz_round_trip() {
    let base = spawn_server(vec![ScriptStep::text("noop")]).await;
    let client = RemoteClient::new(&base);
    let body = client.healthz().await.expect("healthz");
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn chat_round_trip_streams_text_delta_and_done() {
    let base = spawn_server(vec![ScriptStep::text("hello back")]).await;
    let client = RemoteClient::new(&base);

    let session = client
        .create_session(&CreateSessionRequest {
            user_id: "usr_a".into(),
            tenant_id: "ten_a".into(),
            model: "mock".into(),
            title: None,
        })
        .await
        .expect("create");

    let events = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let events_for_cb = events.clone();
    client
        .send_message(&session.id, "hi", move |ev| {
            events_for_cb.lock().push((ev.name, ev.payload));
            Ok(())
        })
        .await
        .expect("send");

    let collected = events.lock();
    let names: Vec<&str> = collected.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"text_delta"), "names = {names:?}");
    assert!(names.contains(&"done"), "names = {names:?}");
    let text: String = collected
        .iter()
        .filter(|(n, _)| n == "text_delta")
        .filter_map(|(_, p)| p.get("delta").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(text, "hello back");
}

#[tokio::test]
async fn messages_endpoint_lists_persisted_history() {
    let base = spawn_server(vec![ScriptStep::text("ok")]).await;
    let client = RemoteClient::new(&base);

    let session = client
        .create_session(&CreateSessionRequest {
            user_id: "usr_a".into(),
            tenant_id: "ten_a".into(),
            model: "mock".into(),
            title: None,
        })
        .await
        .expect("create");
    client
        .send_message(&session.id, "hi", |_| Ok(()))
        .await
        .expect("send");
    // Finalize task persists asynchronously.
    tokio::time::sleep(Duration::from_millis(80)).await;

    let msgs = client.list_messages(&session.id).await.expect("list");
    assert_eq!(msgs.len(), 2, "msgs = {msgs:?}");
}

#[tokio::test]
async fn cancel_returns_false_when_idle() {
    let base = spawn_server(vec![ScriptStep::text("noop")]).await;
    let client = RemoteClient::new(&base);
    let session = client
        .create_session(&CreateSessionRequest {
            user_id: "u".into(),
            tenant_id: "t".into(),
            model: "mock".into(),
            title: None,
        })
        .await
        .expect("create");
    let cancelled = client.cancel(&session.id).await.expect("cancel");
    assert!(!cancelled);
}
