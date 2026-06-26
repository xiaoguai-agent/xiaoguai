//! T7.1 integration tests: team-glossary injection into (a) chat turns —
//! positioned AFTER the identity message and BEFORE history, for execute,
//! consult and loop turns alike — (b) orchestrate member + synthesis runs
//! (right after the persona system messages), and (c) the absent cases
//! (no team / blank-glossary teams never inject).
//!
//! Uses the `incident_pipeline.rs` `RecordingBackend` pattern: a wrapper that
//! records every `ChatRequest` so the test can assert exactly what the
//! model observed. Identity is pinned via `XIAOGUAI_IDENTITY_PATH` — safe
//! under nextest (one process per test).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::orchestrate::OrchestrateMemberRunner;
use xiaoguai_api::{router, run_turn, AppState, CancelRegistry, TurnInput, TurnMode};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{ChatRequest, ChatStream, LlmBackend, LlmError, MockBackend, Role};
use xiaoguai_orchestrator::patterns::executive::{MemberRunner, MemberSpec};
use xiaoguai_personas::teams::model::CreateTeamRequest;
use xiaoguai_personas::{
    CreatePersonaRequest, InMemoryPersonaRepository, InMemoryTeamRepository, PersonaRepository,
    TeamRepository,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

const IDENTITY_TEXT: &str = "I am the owner. Prefer terse answers.";
const GLOSSARY_MD: &str = "MRR = monthly recurring revenue";
const GLOSSARY_MESSAGE: &str = "Team glossary (Finance Squad):\nMRR = monthly recurring revenue";

// ── RecordingBackend (incident_pipeline.rs pattern) ───────────────────────────

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

// ── Fixture ───────────────────────────────────────────────────────────────────

struct Fixture {
    state: AppState,
    backend: Arc<RecordingBackend>,
    personas: Arc<dyn PersonaRepository>,
    teams: Arc<dyn TeamRepository>,
    /// Keeps the identity temp file alive for the test's duration.
    _identity_file: tempfile::NamedTempFile,
}

/// Pin `USER.md` to a temp file so identity injection is deterministic
/// regardless of the developer machine's real `~/.xiaoguai/USER.md`. Safe
/// under nextest (process-per-test); would race under plain `cargo test`.
fn pin_identity() -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().expect("identity temp file");
    std::fs::write(f.path(), IDENTITY_TEXT).expect("write identity");
    std::env::set_var("XIAOGUAI_IDENTITY_PATH", f.path());
    f
}

fn fixture(backend: Arc<RecordingBackend>) -> Fixture {
    let identity_file = pin_identity();
    let personas: Arc<dyn PersonaRepository> = Arc::new(InMemoryPersonaRepository::new());
    let teams: Arc<dyn TeamRepository> = Arc::new(InMemoryTeamRepository::new());
    let state = AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend: backend.clone(),
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
        personas: Some(personas.clone()),
        teams: Some(teams.clone()),
        incidents: None,
        team_audit: None,
        watchers: None,
        loops: None,
        decision_registry: Arc::new(xiaoguai_api::hotl::decision_registry::DecisionRegistry::new()),
        pack_rescanner: None,
    };
    Fixture {
        state,
        backend,
        personas,
        teams,
        _identity_file: identity_file,
    }
}

impl Fixture {
    async fn make_persona(&self, name: &str) -> Uuid {
        self.personas
            .create(&CreatePersonaRequest {
                name: name.to_string(),
                system_prompt: format!("You are {name}."),
                default_model: None,
                tool_allowlist: None,
                escalation_tier: None,
            })
            .await
            .expect("create persona")
            .id
    }

    /// Create a team (with the given glossary) and attach it to `session_id`.
    async fn attach_team(&self, session_id: &str, glossary_md: Option<&str>) {
        let lead = self.make_persona(&format!("Lead-{session_id}")).await;
        let team = self
            .teams
            .create(&CreateTeamRequest {
                name: "Finance Squad".to_string(),
                description: String::new(),
                lead_persona_id: lead,
                member_persona_ids: vec![lead],
                recommended_pack_slugs: vec![],
                glossary_md: glossary_md.map(str::to_string),
            })
            .await
            .expect("create team");
        self.teams
            .attach_team_to_session(session_id, team.id)
            .await
            .expect("attach team");
    }
}

async fn create_session(state: &AppState) -> String {
    let app = router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/sessions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "user_id": "usr_owner", "model": "mock-model" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    v["id"].as_str().unwrap().to_string()
}

/// Run one turn to completion and return the FIRST recorded model request.
async fn run_one_turn(fx: &Fixture, input: TurnInput) -> ChatRequest {
    let handle = run_turn(&fx.state, input).await.expect("turn starts");
    handle.completion.await.expect("turn completes");
    let requests = fx.backend.requests();
    assert!(!requests.is_empty(), "backend saw no requests");
    requests[0].clone()
}

fn turn_input(session_id: &str, mode: TurnMode) -> TurnInput {
    TurnInput {
        session_id: session_id.to_string(),
        content: "hi".to_string(),
        model_override: None,
        mode,
        loop_id: None,
        loop_dynamic_pacing: false,
    }
}

/// Index of the first message whose content equals `needle`, or None.
fn position_of(req: &ChatRequest, needle: &str) -> Option<usize> {
    req.messages.iter().position(|m| m.content == needle)
}

// ── (a) chat turn injection ───────────────────────────────────────────────────

#[tokio::test]
async fn execute_turn_injects_glossary_after_identity_before_history() {
    let fx = fixture(RecordingBackend::new(vec![ScriptStep::text("ok")]));
    let sid = create_session(&fx.state).await;
    fx.attach_team(&sid, Some(GLOSSARY_MD)).await;

    let req = run_one_turn(&fx, turn_input(&sid, TurnMode::Execute)).await;

    let identity = position_of(&req, IDENTITY_TEXT).expect("identity message present");
    let glossary = position_of(&req, GLOSSARY_MESSAGE).expect("glossary message present");
    let user = position_of(&req, "hi").expect("user message present");
    assert!(matches!(req.messages[glossary].role, Role::System));
    assert!(
        identity < glossary && glossary < user,
        "expected [identity({identity}) < glossary({glossary}) < history({user})]"
    );
}

#[tokio::test]
async fn consult_turn_injects_glossary_too() {
    let fx = fixture(RecordingBackend::new(vec![ScriptStep::text("ok")]));
    let sid = create_session(&fx.state).await;
    fx.attach_team(&sid, Some(GLOSSARY_MD)).await;

    let req = run_one_turn(&fx, turn_input(&sid, TurnMode::Consult)).await;

    let identity = position_of(&req, IDENTITY_TEXT).expect("identity message present");
    let glossary = position_of(&req, GLOSSARY_MESSAGE).expect("glossary message present");
    assert!(identity < glossary);
}

#[tokio::test]
async fn loop_turn_injects_glossary_with_identity_outermost() {
    // Loop ticks belong to a session like any other turn (plan §1.1), so the
    // glossary applies; identity stays the outermost System frame:
    // [identity, glossary, loop_note, ...history].
    let fx = fixture(RecordingBackend::new(vec![ScriptStep::text("ok")]));
    let sid = create_session(&fx.state).await;
    fx.attach_team(&sid, Some(GLOSSARY_MD)).await;

    let mut input = turn_input(&sid, TurnMode::Execute);
    input.loop_id = Some(Uuid::new_v4());
    let req = run_one_turn(&fx, input).await;

    let identity = position_of(&req, IDENTITY_TEXT).expect("identity message present");
    let glossary = position_of(&req, GLOSSARY_MESSAGE).expect("glossary message present");
    let loop_note = req
        .messages
        .iter()
        .position(|m| m.content.contains("recurring loop"))
        .expect("loop tick note present");
    assert!(
        identity < glossary && glossary < loop_note,
        "expected [identity({identity}) < glossary({glossary}) < loop_note({loop_note})]"
    );
}

// ── (c) absent cases ──────────────────────────────────────────────────────────

#[tokio::test]
async fn no_team_means_no_glossary_message() {
    let fx = fixture(RecordingBackend::new(vec![ScriptStep::text("ok")]));
    let sid = create_session(&fx.state).await;

    let req = run_one_turn(&fx, turn_input(&sid, TurnMode::Execute)).await;
    assert!(
        !req.messages
            .iter()
            .any(|m| m.content.contains("Team glossary")),
        "no team attached → no glossary injection"
    );
}

#[tokio::test]
async fn team_without_glossary_injects_nothing() {
    let fx = fixture(RecordingBackend::new(vec![ScriptStep::text("ok")]));
    let sid = create_session(&fx.state).await;
    fx.attach_team(&sid, None).await;

    let req = run_one_turn(&fx, turn_input(&sid, TurnMode::Execute)).await;
    assert!(
        !req.messages
            .iter()
            .any(|m| m.content.contains("Team glossary")),
        "blank/absent glossary → no injection"
    );
}

// ── (b) orchestrate member + synthesis runs ───────────────────────────────────

fn member_runner(
    backend: Arc<RecordingBackend>,
    persona_id: Uuid,
    glossary: Option<String>,
) -> OrchestrateMemberRunner {
    let persona = xiaoguai_personas::Persona {
        id: persona_id,
        name: "Analyst".to_string(),
        system_prompt: "You are Analyst.".to_string(),
        default_model: None,
        tool_allowlist: None,
        escalation_tier: None,
        created_at: chrono::Utc::now(),
        archived: false,
    };
    let personas: HashMap<Uuid, xiaoguai_personas::Persona> =
        [(persona_id, persona)].into_iter().collect();
    OrchestrateMemberRunner::new(
        backend,
        Arc::new(Toolbox::new()),
        AgentConfig::new("mock-model"),
        personas,
        "mock-model".to_string(),
        "owner".to_string(),
        Uuid::new_v4(),
        CancellationToken::new(),
        glossary,
    )
}

#[tokio::test]
async fn orchestrate_member_run_carries_glossary_after_persona_prompt() {
    let backend = RecordingBackend::new(vec![ScriptStep::text("finding")]);
    let persona_id = Uuid::new_v4();
    let runner = member_runner(
        backend.clone(),
        persona_id,
        Some(GLOSSARY_MESSAGE.to_string()),
    );

    let spec = MemberSpec {
        id: persona_id,
        name: "Analyst".to_string(),
    };
    runner
        .run_member(&spec, "assess Q2")
        .await
        .expect("member run");

    let req = &backend.requests()[0];
    let persona = position_of(req, "You are Analyst.").expect("persona prompt present");
    let glossary = position_of(req, GLOSSARY_MESSAGE).expect("glossary present");
    let goal = position_of(req, "assess Q2").expect("goal present");
    assert!(matches!(req.messages[glossary].role, Role::System));
    assert!(
        persona < glossary && glossary < goal,
        "persona prompt leads, glossary right after, then the user goal"
    );
}

#[tokio::test]
async fn orchestrate_synthesis_run_carries_glossary_too() {
    let backend = RecordingBackend::new(vec![ScriptStep::text("synthesis")]);
    let persona_id = Uuid::new_v4();
    let runner = member_runner(
        backend.clone(),
        persona_id,
        Some(GLOSSARY_MESSAGE.to_string()),
    );

    let lead = MemberSpec {
        id: persona_id,
        name: "Analyst".to_string(),
    };
    runner
        .run_synthesis(&lead, "assess Q2", &[])
        .await
        .expect("synthesis run");

    let req = &backend.requests()[0];
    assert!(position_of(req, GLOSSARY_MESSAGE).is_some());
}

#[tokio::test]
async fn orchestrate_member_run_without_glossary_has_no_glossary_message() {
    let backend = RecordingBackend::new(vec![ScriptStep::text("finding")]);
    let persona_id = Uuid::new_v4();
    let runner = member_runner(backend.clone(), persona_id, None);

    let spec = MemberSpec {
        id: persona_id,
        name: "Analyst".to_string(),
    };
    runner
        .run_member(&spec, "assess Q2")
        .await
        .expect("member run");

    let req = &backend.requests()[0];
    assert!(
        !req.messages
            .iter()
            .any(|m| m.content.contains("Team glossary")),
        "no glossary configured → member request carries none"
    );
}
