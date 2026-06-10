//! T5.1 consult/execute integration tests.
//!
//! Proves the two enforcement layers independently (plan §2.2/§2.3):
//! * Layer 1 (visibility): a consult turn's toolbox is the read-only
//!   subset — a write tool the model names anyway is "not in toolbox".
//! * Layer 2 (ConsultGate): with the FULL toolbox + the gate (the
//!   defense-in-depth config), the write call is denied with the stable
//!   consult reason before the MCP client is touched.
//!
//! Plus: execute turns are unchanged, and the `agent.run` audit entry
//! carries `"mode"` (`consult` / default `execute`).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, ConsultGate, ReactAgent, Toolbox, CONSULT_DENY_REASON};
use xiaoguai_api::consult::read_only_tool_names;
use xiaoguai_api::hotl::audit::InMemoryHotlAuditSink;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message as LlmMessage, MockBackend, ToolCallSpec};
use xiaoguai_mcp::{McpClient, McpResult, MutationHint, ServerInfo, ToolDescriptor, ToolResult};

/// MCP client that counts invocations per tool — proves a denied/hidden
/// write tool was never dispatched.
#[derive(Debug, Default)]
struct CountingClient {
    calls: AtomicUsize,
}

#[async_trait]
impl McpClient for CountingClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "counting".into(),
            version: "0".into(),
        })
    }
    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    async fn call_tool(&self, name: &str, _args: Value) -> McpResult<ToolResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            text: format!("{name} executed"),
            blocks: vec![],
            is_error: false,
        })
    }
    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

fn td(name: &str, hint: MutationHint) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: Some(format!("tool {name}")),
        input_schema: json!({ "type": "object" }),
        mutation_hint: hint,
    }
}

/// One read tool (`lookup`) + one write tool (`mutate`), both owned by the
/// same counting client (writes counted via `write_client`).
fn fixture_toolbox(
    read_client: &Arc<CountingClient>,
    write_client: &Arc<CountingClient>,
) -> Toolbox {
    let mut tb = Toolbox::new();
    tb.insert(
        read_client.clone() as Arc<dyn McpClient>,
        td("lookup", MutationHint::Read),
    )
    .expect("insert lookup");
    tb.insert(
        write_client.clone() as Arc<dyn McpClient>,
        td("mutate", MutationHint::Write),
    )
    .expect("insert mutate");
    tb
}

fn tool_call(name: &str) -> ToolCallSpec {
    ToolCallSpec {
        id: format!("call-{name}"),
        name: name.into(),
        arguments_json: "{}".into(),
    }
}

struct Fixture {
    state: AppState,
    audit: Arc<InMemoryHotlAuditSink>,
    write_client: Arc<CountingClient>,
}

fn build_fixture(backend: Arc<dyn LlmBackend>) -> Fixture {
    let read_client = Arc::new(CountingClient::default());
    let write_client = Arc::new(CountingClient::default());
    let audit = Arc::new(InMemoryHotlAuditSink::new());
    let state = AppState {
        sessions: common::InMemorySessionRepo::arc(),
        messages: common::InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(fixture_toolbox(&read_client, &write_client)),
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
        hotl_audit: Some(audit.clone()),
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
        team_audit: None,
        decision_registry: Arc::new(xiaoguai_api::hotl::decision_registry::DecisionRegistry::new()),
    };
    Fixture {
        state,
        audit,
        write_client,
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

async fn body_to_string(body: Body) -> String {
    let bytes = tokio::time::timeout(Duration::from_secs(10), to_bytes(body, 4 * 1024 * 1024))
        .await
        .expect("SSE stream did not close within 10s")
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn create_session(app: &axum::Router) -> String {
    let resp = app
        .clone()
        .oneshot(json_post(
            "/v1/sessions",
            &json!({ "user_id": "usr_owner", "model": "mock-model" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = serde_json::from_str(&body_to_string(resp.into_body()).await).unwrap();
    v["id"].as_str().unwrap().to_string()
}

/// Wait for the detached finalize task to release the turn lock — only
/// then is the `agent.run` audit entry guaranteed to be appended.
async fn wait_for_turn_finalized(state: &AppState, sid: &str) {
    for _ in 0..500 {
        if !state.cancels.is_active(sid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("turn for {sid} never finalized");
}

/// Run one full turn through the route and return the drained SSE body.
async fn run_route_turn(fixture: &Fixture, body: Value) -> String {
    let app = router(fixture.state.clone());
    let sid = create_session(&app).await;
    let resp = app
        .clone()
        .oneshot(json_post(&format!("/v1/sessions/{sid}/messages"), &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let sse = body_to_string(resp.into_body()).await;
    wait_for_turn_finalized(&fixture.state, &sid).await;
    sse
}

fn agent_run_entries(audit: &InMemoryHotlAuditSink) -> Vec<xiaoguai_audit::AuditEntry> {
    audit
        .snapshot()
        .into_iter()
        .filter(|e| e.action == "agent.run")
        .collect()
}

// ── (a) layer 1: consult toolbox subset ─────────────────────────────────────

#[tokio::test]
async fn consult_turn_blocks_write_tools_named_by_the_model() {
    // The model hallucinates the write tool anyway. Layer 1 already hid it
    // (subset toolbox — pinned by the consult.rs unit tests), and layer 2's
    // ConsultGate fires before dispatch, so the synthesized tool failure the
    // model observes carries the stable consult-deny reason. Either way the
    // write client is NEVER invoked.
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![tool_call("mutate")]),
        ScriptStep::text("done"),
    ]));
    let fixture = build_fixture(backend);

    let sse = run_route_turn(
        &fixture,
        json!({ "content": "try to write", "mode": "consult" }),
    )
    .await;

    assert!(
        sse.contains(CONSULT_DENY_REASON),
        "hallucinated write tool must fail with the consult reason, got SSE: {sse}"
    );
    assert_eq!(
        fixture.write_client.calls.load(Ordering::SeqCst),
        0,
        "write tool must never be dispatched in consult mode"
    );
}

#[tokio::test]
async fn consult_turn_still_runs_read_tools() {
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![tool_call("lookup")]),
        ScriptStep::text("done"),
    ]));
    let fixture = build_fixture(backend);

    let sse = run_route_turn(
        &fixture,
        json!({ "content": "look something up", "mode": "consult" }),
    )
    .await;

    assert!(
        sse.contains("lookup executed"),
        "read tool must work in consult mode, got SSE: {sse}"
    );
}

// ── (a) layer 2: ConsultGate denies even with the full toolbox ──────────────

#[tokio::test]
async fn consult_gate_denies_write_tool_even_when_toolbox_contains_it() {
    // Defense-in-depth: bypass layer 1 on purpose (full toolbox) and prove
    // the gate alone blocks the write call with the consult reason.
    let read_client = Arc::new(CountingClient::default());
    let write_client = Arc::new(CountingClient::default());
    let toolbox = fixture_toolbox(&read_client, &write_client);
    let read_set = read_only_tool_names(&toolbox);

    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![tool_call("mutate")]),
        ScriptStep::text("done"),
    ]));
    let config =
        AgentConfig::new("mock-model").with_hotl_gate(Arc::new(ConsultGate::new(None, read_set)));
    let agent = ReactAgent::new(backend, toolbox, config);

    let (_outcome, events) = agent
        .run_to_completion(
            vec![LlmMessage::user("try to write")],
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("run completes");

    let denied = events.iter().any(|e| {
        matches!(
            e,
            xiaoguai_agent::AgentEvent::ToolCallFinished { ok: false, error: Some(err), .. }
                if err.contains(CONSULT_DENY_REASON)
        )
    });
    assert!(
        denied,
        "gate must deny the write tool with the consult reason, events: {events:?}"
    );
    assert_eq!(
        write_client.calls.load(Ordering::SeqCst),
        0,
        "denied tool must never reach its MCP client"
    );
}

// ── (b) execute turns unchanged ─────────────────────────────────────────────

#[tokio::test]
async fn execute_turn_can_call_write_tools() {
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![tool_call("mutate")]),
        ScriptStep::text("done"),
    ]));
    let fixture = build_fixture(backend);

    // No `mode` at all — the default execute path.
    let sse = run_route_turn(&fixture, json!({ "content": "write something" })).await;

    assert!(
        sse.contains("mutate executed"),
        "write tool must run in execute mode, got SSE: {sse}"
    );
    assert_eq!(fixture.write_client.calls.load(Ordering::SeqCst), 1);
}

// ── (c)/(d) audit carries the mode ──────────────────────────────────────────

#[tokio::test]
async fn consult_mode_is_stamped_into_agent_run_audit() {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("hi")]));
    let fixture = build_fixture(backend);

    run_route_turn(&fixture, json!({ "content": "hello", "mode": "consult" })).await;

    let entries = agent_run_entries(&fixture.audit);
    assert_eq!(entries.len(), 1, "exactly one agent.run entry");
    assert_eq!(entries[0].details["mode"], "consult");
}

#[tokio::test]
async fn default_mode_audits_as_execute() {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("hi")]));
    let fixture = build_fixture(backend);

    run_route_turn(&fixture, json!({ "content": "hello" })).await;

    let entries = agent_run_entries(&fixture.audit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].details["mode"], "execute");
}

// ── loop turns ignore mode (plan §2.4 guard) ────────────────────────────────

#[tokio::test]
async fn loop_turns_ignore_consult_mode_and_stay_execute() {
    // A loop tick that (wrongly) carries mode=consult must still run
    // execute: write tools dispatch, audit says "execute".
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![tool_call("mutate")]),
        ScriptStep::text("done"),
    ]));
    let fixture = build_fixture(backend);
    let app = router(fixture.state.clone());
    let sid = create_session(&app).await;

    let handle = xiaoguai_api::run_turn(
        &fixture.state,
        xiaoguai_api::TurnInput {
            session_id: sid.clone(),
            content: "loop tick".into(),
            model_override: None,
            mode: xiaoguai_api::TurnMode::Consult, // must be ignored
            loop_id: Some(uuid::Uuid::new_v4()),
            loop_dynamic_pacing: false,
        },
    )
    .await
    .expect("loop turn starts");
    drop(handle.events);
    handle.completion.await.expect("turn completes");
    wait_for_turn_finalized(&fixture.state, &sid).await;

    assert_eq!(
        fixture.write_client.calls.load(Ordering::SeqCst),
        1,
        "loop turns must stay execute — write tool dispatches"
    );
    let entries = agent_run_entries(&fixture.audit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].details["mode"], "execute");
    assert_eq!(entries[0].details["initiator"], "loop");
}

#[tokio::test]
async fn explicit_execute_mode_is_accepted_and_audited() {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("hi")]));
    let fixture = build_fixture(backend);

    run_route_turn(&fixture, json!({ "content": "hello", "mode": "execute" })).await;

    let entries = agent_run_entries(&fixture.audit);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].details["mode"], "execute");
}
