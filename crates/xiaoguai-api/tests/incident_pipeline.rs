//! Integration tests for the T6.3/T6.4 self-healing pipeline:
//! `POST /v1/incidents/{id}/analyze`, `POST /v1/incidents/{id}/approve-repair`,
//! `GET /v1/incidents/{id}/report`.
//!
//! Boots the production router with the in-memory incident store and a
//! scripted [`MockBackend`] for the agent turns. Covers: the full happy
//! path (ingest → analyze → approve → resolved), the parse-failure revert
//! to `open`, the 409 transition guards, the markdown report, and the
//! consult lock on the Analyst turn (write tool never dispatched; the tool
//! result the model observes carries the stable consult-deny reason).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox, CONSULT_DENY_REASON};
use xiaoguai_api::hotl::audit::InMemoryHotlAuditSink;
use xiaoguai_api::incident_store::{IncidentStatus, IncidentStore};
use xiaoguai_api::{
    router, AppState, CancelRegistry, InMemoryIncidentStore, StaticWebhookTokenValidator,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{
    ChatRequest, ChatStream, LlmBackend, LlmError, MockBackend, Role, ToolCallSpec,
};
use xiaoguai_mcp::{McpClient, McpResult, MutationHint, ServerInfo, ToolDescriptor, ToolResult};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

const TOKEN: &str = "tok-incidents-1";
const TOKEN_HEADER: &str = "X-Xiaoguai-Token";

// ── Backend wrapper that records every ChatRequest ───────────────────────────

/// Delegates to an inner backend and records each request's messages +
/// attribution — lets tests assert what the model actually observed
/// (tool-result content, `incident:<id>` session label).
struct RecordingBackend {
    inner: MockBackend,
    requests: Mutex<Vec<ChatRequest>>,
}

impl RecordingBackend {
    fn new(steps: Vec<ScriptStep>) -> Arc<Self> {
        Arc::new(Self {
            inner: MockBackend::with_script(steps),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait]
impl LlmBackend for RecordingBackend {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.requests
            .lock()
            .expect("requests lock")
            .push(req.clone());
        self.inner.chat_stream(req).await
    }

    fn name(&self) -> &'static str {
        "recording-mock"
    }
}

// ── Counting MCP client (consult_mode.rs pattern) ─────────────────────────────

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

/// One read tool (`lookup`) + one write tool (`mutate`).
fn rw_toolbox(read_client: &Arc<CountingClient>, write_client: &Arc<CountingClient>) -> Toolbox {
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

// ── Fixture (tests/incidents.rs style, parameterized backend/toolbox) ────────

struct Fixture {
    incidents: Arc<InMemoryIncidentStore>,
    audit: Arc<InMemoryHotlAuditSink>,
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
}

impl Fixture {
    fn new(backend: Arc<dyn LlmBackend>, toolbox: Toolbox) -> Self {
        Self {
            incidents: Arc::new(InMemoryIncidentStore::new()),
            audit: Arc::new(InMemoryHotlAuditSink::new()),
            backend,
            toolbox: Arc::new(toolbox),
        }
    }

    fn scripted(steps: Vec<ScriptStep>) -> Self {
        Self::new(Arc::new(MockBackend::with_script(steps)), Toolbox::new())
    }

    fn state(&self) -> AppState {
        AppState {
            sessions: InMemorySessionRepo::arc(),
            messages: InMemoryMessageRepo::arc(),
            backend: self.backend.clone(),
            toolbox: self.toolbox.clone(),
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
            webhook_token_validator: Some(Arc::new(StaticWebhookTokenValidator {
                token: TOKEN.to_string(),
                route_id: "incidents".to_string(),
            })),
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
            teams: None,
            incidents: Some(self.incidents.clone()),
            team_audit: Some(self.audit.clone()),
            watchers: None,
            loops: None,
            decision_registry: Arc::new(
                xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
            ),
        }
    }

    fn audit_actions(&self) -> Vec<String> {
        self.audit
            .snapshot()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(TOKEN_HEADER, t);
    }
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn send_json(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, text) = send(app, method, uri, token, body).await;
    let json = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, json)
}

fn sentry_payload() -> Value {
    json!({
        "action": "created",
        "data": {
            "issue": {
                "id": "123",
                "title": "ZeroDivisionError: division by zero",
                "level": "error",
                "firstSeen": "2026-06-10T01:02:03.000Z",
                "permalink": "https://sentry.io/organizations/acme/issues/123/",
                "project": {"slug": "backend"},
                "tags": [{"key": "environment", "value": "production"}]
            }
        }
    })
}

/// A reply matching the `RcaDraft` serde contract exactly.
fn rca_reply() -> String {
    json!({
        "summary": "Payment processor crashed on empty carts.",
        "impact": "200 users for 12 minutes.",
        "root_cause": "Divide-by-zero in discount_calc.",
        "timeline": [{"time": "2026-06-10T01:00:00Z", "event": "deploy v1.2.4"}],
        "action_items": [
            {"assignee": "backend", "action": "Add zero-cart guard", "priority": "P0"}
        ],
        "confidence": "high",
        "evidence_refs": ["commit:abc123"]
    })
    .to_string()
}

/// Run analyze and return the persisted RCA id (#284: approve-repair now
/// requires the `rca_id` being approved).
async fn analyze_ok(app: &axum::Router, id: Uuid) -> Uuid {
    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "analyze failed: {body}");
    body["rca"]["id"].as_str().unwrap().parse().unwrap()
}

/// Ingest the fixture sentry alert and return the incident id.
async fn ingest(app: &axum::Router) -> Uuid {
    let (status, body) = send_json(
        app.clone(),
        "POST",
        "/v1/incidents/ingest/sentry",
        Some(TOKEN),
        Some(sentry_payload()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "ingest failed: {body}");
    body["incident"]["id"].as_str().unwrap().parse().unwrap()
}

// ── Happy path: analyze ───────────────────────────────────────────────────────

#[tokio::test]
async fn analyze_persists_rca_and_moves_to_awaiting_approval() {
    let fx = Fixture::scripted(vec![ScriptStep::text(rca_reply())]);
    let app = router(fx.state());
    let id = ingest(&app).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], "awaiting_approval");
    assert_eq!(
        body["rca"]["summary"],
        "Payment processor crashed on empty carts."
    );
    assert_eq!(
        body["rca"]["root_cause"],
        "Divide-by-zero in discount_calc."
    );
    // Attribution label is stamped into the RCA's session_id.
    assert_eq!(body["rca"]["session_id"], format!("incident:{id}"));
    // Qualitative "high" landed as the numeric column.
    assert!((body["rca"]["confidence"].as_f64().unwrap() - 0.9).abs() < 1e-9);

    // Store: status + RCA row persisted.
    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::AwaitingApproval);
    assert_eq!(details.rcas.len(), 1);
    assert_eq!(details.rcas[0].raw_markdown, rca_reply());

    // Audit: open + analyzed.
    assert_eq!(
        fx.audit_actions(),
        vec!["incident.open", "incident.analyzed"]
    );
}

#[tokio::test]
async fn analyze_stamps_incident_attribution_on_the_model_call() {
    let backend = RecordingBackend::new(vec![ScriptStep::text(rca_reply())]);
    let fx = Fixture::new(backend.clone(), Toolbox::new());
    let app = router(fx.state());
    let id = ingest(&app).await;

    let (status, _) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let requests = backend.requests();
    assert!(!requests.is_empty());
    for req in &requests {
        assert_eq!(req.session_id.as_deref(), Some(&*format!("incident:{id}")));
    }
}

// ── Happy path: approve-repair ────────────────────────────────────────────────

#[tokio::test]
async fn approve_repair_records_repair_and_resolves() {
    // Step 1 feeds the Analyst turn, step 2 the Executor turn.
    let fx = Fixture::scripted(vec![
        ScriptStep::text(rca_reply()),
        ScriptStep::text("Checkpointed, added the zero-cart guard, tests green."),
    ]);
    let app = router(fx.state());
    let id = ingest(&app).await;
    let rca_id = analyze_ok(&app, id).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": rca_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], "resolved");
    assert_eq!(body["repair"]["ok"], true);
    assert_eq!(
        body["repair"]["summary"],
        "Checkpointed, added the zero-cart guard, tests green."
    );
    assert_eq!(body["repair"]["session_id"], format!("incident:{id}"));

    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::Resolved);
    assert_eq!(details.repairs.len(), 1);
    assert!(details.repairs[0].ok);
    assert_eq!(details.repairs[0].rca_id, details.rcas[0].id);

    assert_eq!(
        fx.audit_actions(),
        vec!["incident.open", "incident.analyzed", "incident.repaired"]
    );
}

// ── Analysis failure path ─────────────────────────────────────────────────────

#[tokio::test]
async fn unparseable_analyst_reply_reverts_to_open_and_audits_failure() {
    let fx = Fixture::scripted(vec![ScriptStep::text(
        "I am unable to determine the root cause.",
    )]);
    let app = router(fx.state());
    let id = ingest(&app).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body: {body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("did not match the RCA contract"));

    // Reverted to open (retryable), no RCA row.
    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::Open);
    assert!(details.rcas.is_empty());

    // Failure is audited with a reason.
    let entries = fx.audit.snapshot();
    let failed: Vec<_> = entries
        .iter()
        .filter(|e| e.action == "incident.analysis_failed")
        .collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].details["reason"]
        .as_str()
        .unwrap()
        .contains("JSON"));

    // …and a retry with the same (replayed) script fails the same way,
    // proving the open state is genuinely retryable.
    let (status, _) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

// ── Transition guards (409) ───────────────────────────────────────────────────

#[tokio::test]
async fn analyze_on_already_analyzing_incident_returns_409() {
    let fx = Fixture::scripted(vec![ScriptStep::text(rca_reply())]);
    let app = router(fx.state());
    let id = ingest(&app).await;
    fx.incidents
        .set_status(id, IncidentStatus::Analyzing)
        .await
        .unwrap();

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("illegal status transition"));
}

#[tokio::test]
async fn approve_repair_on_open_incident_returns_409() {
    let fx = Fixture::scripted(vec![ScriptStep::text("never used")]);
    let app = router(fx.state());
    let id = ingest(&app).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": Uuid::new_v4()})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    // No repair row, status untouched.
    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::Open);
    assert!(details.repairs.is_empty());
}

// ── #284: approval binds to the RCA, not the incident ────────────────────────

#[tokio::test]
async fn approve_repair_with_stale_rca_id_returns_409_and_changes_nothing() {
    // Step 1 feeds the Analyst; step 2 is only reached by the FINAL
    // (correct rca_id) approval below — the stale approval in between
    // must never start an Executor turn.
    let fx = Fixture::scripted(vec![
        ScriptStep::text(rca_reply()),
        ScriptStep::text("Executor ran after the correct rca_id approval."),
    ]);
    let app = router(fx.state());
    let id = ingest(&app).await;
    let _latest = analyze_ok(&app, id).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": Uuid::new_v4()})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert!(
        body["message"].as_str().unwrap().contains("latest RCA"),
        "body: {body}"
    );

    // Nothing transitioned, no repair recorded, no Executor turn ran.
    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::AwaitingApproval);
    assert!(details.repairs.is_empty());

    // The CORRECT (latest) rca_id still goes through afterwards.
    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": details.rcas[0].id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], "resolved");
}

#[tokio::test]
async fn approve_repair_without_rca_id_returns_400() {
    let fx = Fixture::scripted(vec![ScriptStep::text(rca_reply())]);
    let app = router(fx.state());
    let id = ingest(&app).await;
    analyze_ok(&app, id).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body["message"].as_str().unwrap().contains("rca_id"));
    // Still awaiting approval — nothing moved.
    let details = fx.incidents.get_with_details(id).await.unwrap();
    assert_eq!(details.incident.status, IncidentStatus::AwaitingApproval);
}

#[tokio::test]
async fn pipeline_routes_return_503_when_store_absent_and_404_on_unknown_id() {
    let fx = Fixture::scripted(vec![ScriptStep::text("noop")]);
    let mut state = fx.state();
    state.incidents = None;
    let app = router(state);
    for (method, path) in [
        ("POST", format!("/v1/incidents/{}/analyze", Uuid::new_v4())),
        (
            "POST",
            format!("/v1/incidents/{}/approve-repair", Uuid::new_v4()),
        ),
        ("GET", format!("/v1/incidents/{}/report", Uuid::new_v4())),
    ] {
        let (status, _) = send_json(app.clone(), method, &path, None, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
    }

    let app = router(fx.state());
    let (status, _) = send_json(
        app,
        "POST",
        &format!("/v1/incidents/{}/analyze", Uuid::new_v4()),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Report ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn report_renders_markdown_with_title_rca_and_repairs() {
    let fx = Fixture::scripted(vec![
        ScriptStep::text(rca_reply()),
        ScriptStep::text("Guard added and deployed."),
    ]);
    let app = router(fx.state());
    let id = ingest(&app).await;

    // Before any RCA: still renders, says so.
    let (status, md) = send(
        app.clone(),
        "GET",
        &format!("/v1/incidents/{id}/report"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(md.contains("ZeroDivisionError: division by zero"));
    assert!(md.contains("No RCA recorded yet"));

    let rca_id = analyze_ok(&app, id).await;
    send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": rca_id})),
    )
    .await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/incidents/{id}/report"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.starts_with("text/markdown"), "{content_type}");
    let md = String::from_utf8(
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();

    // Incident title + RCA summary via the existing renderer's sections.
    assert!(md.contains("# Incident RCA: ZeroDivisionError: division by zero"));
    assert!(md.contains("## 1. Summary"));
    assert!(md.contains("Payment processor crashed on empty carts."));
    assert!(md.contains("## 5. Action Items"));
    assert!(md.contains("Add zero-cart guard"));
    // Composed repairs section + final status.
    assert!(md.contains("## 6. Repairs"));
    assert!(md.contains("Guard added and deployed."));
    assert!(md.contains("> **Status**: resolved"));
}

// ── Consult enforcement on the Analyst turn ───────────────────────────────────

#[tokio::test]
async fn analyst_turn_is_consult_locked_write_tool_denied_with_reason() {
    // The Analyst "hallucinates" the write tool, observes the denial, and
    // recovers with a valid RCA. The write client must never be invoked,
    // and the tool result fed back to the model must carry the stable
    // consult-deny reason (asserted via the recorded second ChatRequest).
    let read_client = Arc::new(CountingClient::default());
    let write_client = Arc::new(CountingClient::default());
    let toolbox = rw_toolbox(&read_client, &write_client);
    let backend = RecordingBackend::new(vec![
        ScriptStep::tool_calls(vec![tool_call("mutate")]),
        ScriptStep::text(rca_reply()),
    ]);
    let fx = Fixture::new(backend.clone(), toolbox);
    let app = router(fx.state());
    let id = ingest(&app).await;

    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/analyze"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    assert_eq!(
        write_client.calls.load(Ordering::SeqCst),
        0,
        "write tool must never be dispatched during the Analyst turn"
    );

    // The model's second call saw a tool-result message carrying the
    // consult-deny reason.
    let requests = backend.requests();
    assert!(requests.len() >= 2, "expected a follow-up model call");
    let denied_result_seen = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .any(|m| matches!(m.role, Role::Tool) && m.content.contains(CONSULT_DENY_REASON));
    assert!(
        denied_result_seen,
        "tool result must carry the consult deny reason; messages: {:#?}",
        requests.last().unwrap().messages
    );
}

#[tokio::test]
async fn executor_turn_runs_in_execute_mode_write_tools_allowed() {
    // Counterpart guard: after approval, the Executor CAN dispatch the
    // write tool (full toolbox, no ConsultGate).
    let read_client = Arc::new(CountingClient::default());
    let write_client = Arc::new(CountingClient::default());
    let toolbox = rw_toolbox(&read_client, &write_client);
    let backend = RecordingBackend::new(vec![
        ScriptStep::text(rca_reply()),                     // Analyst
        ScriptStep::tool_calls(vec![tool_call("mutate")]), // Executor mutates…
        ScriptStep::text("Patched and verified."),         // …and reports.
    ]);
    let fx = Fixture::new(backend, toolbox);
    let app = router(fx.state());
    let id = ingest(&app).await;

    let rca_id = analyze_ok(&app, id).await;
    let (status, body) = send_json(
        app.clone(),
        "POST",
        &format!("/v1/incidents/{id}/approve-repair"),
        None,
        Some(json!({"rca_id": rca_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], "resolved");
    assert_eq!(
        write_client.calls.load(Ordering::SeqCst),
        1,
        "executor write tool must dispatch exactly once"
    );
}
