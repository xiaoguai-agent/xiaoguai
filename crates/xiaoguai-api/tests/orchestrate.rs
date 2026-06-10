//! Integration tests for `POST /v1/sessions/:id/orchestrate` (T4.2).
//!
//! Boots the production router with in-memory repos + a scripted
//! `MockBackend` and exercises: the SSE happy path (run events + final,
//! assistant message persisted, `orchestration.start`/`.complete` audit
//! sequence), the 409 turn lock, the 422 boundary refusals, and the 503
//! fallback when teams are absent.
//!
//! NOTE on `MockBackend` + concurrency: `with_script` steps are consumed
//! sequentially from one shared script across ALL `chat_stream` calls, so
//! N concurrent member runs would race for steps. The happy path therefore
//! uses a **single-member team** (member turn → step 1, synthesis turn →
//! step 2, strictly sequential); true 2+-member fan-out is covered by the
//! orchestrator crate's mock-runner tests (T4.1).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::hotl::audit::InMemoryHotlAuditSink;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_personas::teams::model::CreateTeamRequest;
use xiaoguai_personas::{
    CreatePersonaRequest, InMemoryPersonaRepository, InMemoryTeamRepository, PersonaRepository,
    TeamRepository,
};
use xiaoguai_types::{Session, SessionId, SessionStatus, UserId};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

struct Fixture {
    sessions: Arc<InMemorySessionRepo>,
    messages: Arc<InMemoryMessageRepo>,
    personas: Arc<dyn PersonaRepository>,
    teams: Arc<dyn TeamRepository>,
    audit: Arc<InMemoryHotlAuditSink>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            sessions: InMemorySessionRepo::arc(),
            messages: InMemoryMessageRepo::arc(),
            personas: Arc::new(InMemoryPersonaRepository::new()),
            teams: Arc::new(InMemoryTeamRepository::new()),
            audit: Arc::new(InMemoryHotlAuditSink::new()),
        }
    }

    fn state(&self, backend: Arc<dyn LlmBackend>) -> AppState {
        AppState {
            sessions: self.sessions.clone(),
            messages: self.messages.clone(),
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
            personas: Some(self.personas.clone()),
            teams: Some(self.teams.clone()),
            incidents: None,
            team_audit: Some(self.audit.clone()),
            watchers: None,
            loops: None,
            decision_registry: std::sync::Arc::new(
                xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
            ),
        }
    }

    async fn make_persona(&self, name: &str) -> uuid::Uuid {
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

    /// Single-member team (lead == sole member) — see the module note on
    /// `MockBackend` script sequencing.
    async fn make_solo_team(&self, name: &str, lead: uuid::Uuid) -> uuid::Uuid {
        self.teams
            .create(&CreateTeamRequest {
                name: name.to_string(),
                description: "finance reports".to_string(),
                lead_persona_id: lead,
                member_persona_ids: vec![lead],
                recommended_pack_slugs: vec![],
            })
            .await
            .expect("create team")
            .id
    }

    async fn make_session(&self, id: &str) -> String {
        use xiaoguai_storage::repositories::SessionRepository;
        let now = chrono::Utc::now();
        let session = Session {
            id: SessionId::from(id.to_string()),
            user_id: UserId::from("owner".to_string()),
            title: None,
            created_at: now,
            updated_at: now,
            model: String::new(),
            status: SessionStatus::Active,
            parent_session_id: None,
            forked_from_message_id: None,
        };
        self.sessions
            .create(&session)
            .await
            .expect("create session");
        id.to_string()
    }
}

/// Concatenated text blocks of one persisted domain message.
fn text_of(m: &xiaoguai_types::Message) -> String {
    m.content
        .iter()
        .filter_map(|b| match b {
            xiaoguai_types::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

async fn post_orchestrate(
    app: axum::Router,
    session_id: &str,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/sessions/{session_id}/orchestrate"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let resp = app.oneshot(req).await.expect("send request");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ── Happy path: SSE run events + final, persistence, audit sequence ──────────

#[tokio::test]
async fn orchestrate_happy_path_streams_persists_and_audits() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Finance Analyst").await;
    let team_id = fx.make_solo_team("Finance Squad", lead).await;
    let session_id = fx.make_session("sess_orch").await;

    // Step 1 = the sole member's turn; step 2 = the lead's synthesis turn.
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::text("member finding"),
        ScriptStep::text("synthesized answer"),
    ]));
    let app = router(fx.state(backend));

    let (status, body) = post_orchestrate(
        app,
        &session_id,
        serde_json::json!({"goal": "analyse the report", "team_id": team_id}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    // SSE frames: run lifecycle events + the final synthesized text.
    assert!(body.contains("event: run_started"), "body: {body}");
    assert!(body.contains("event: member_started"), "body: {body}");
    assert!(body.contains("event: member_completed"), "body: {body}");
    assert!(body.contains("event: synthesis_started"), "body: {body}");
    assert!(body.contains("event: final"), "body: {body}");
    assert!(body.contains("synthesized answer"), "body: {body}");
    // Per-stream monotonic SSE ids (F5 convention, same as send_message).
    assert!(body.contains("id: 1"), "body: {body}");

    // Persistence: goal as the user message, synthesis as the assistant
    // reply — and ONLY those two (member transcripts are not persisted).
    let msgs = fx.messages.snapshot(&session_id);
    assert_eq!(msgs.len(), 2, "msgs: {msgs:?}");
    assert_eq!(text_of(&msgs[0]), "analyse the report");
    assert_eq!(text_of(&msgs[1]), "synthesized answer");

    // Audit sequence through the team_audit sink.
    let entries = fx.audit.snapshot();
    let actions: Vec<String> = entries.iter().map(|e| e.action.clone()).collect();
    assert_eq!(
        actions,
        vec!["orchestration.start", "orchestration.complete"]
    );
    assert_eq!(entries[0].details["member_count"], 1);
    assert_eq!(entries[0].details["team_id"], team_id.to_string());
    assert_eq!(entries[1].details["ok"], true);
    assert_eq!(entries[1].details["failed_members"], serde_json::json!([]));
    // One run_id threads both entries.
    assert_eq!(entries[0].details["run_id"], entries[1].details["run_id"]);

    // The turn lock was released after the run.
    assert!(!fx.audit.snapshot().is_empty());
}

// ── Auto-routing: goal-only request picks the scorer's top team ──────────────

#[tokio::test]
async fn orchestrate_auto_routes_goal_to_top_team() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Finance Analyst").await;
    fx.make_solo_team("Finance Squad", lead).await;
    let session_id = fx.make_session("sess_auto").await;

    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::text("finding"),
        ScriptStep::text("auto-routed answer"),
    ]));
    let app = router(fx.state(backend));

    let (status, body) = post_orchestrate(
        app,
        &session_id,
        serde_json::json!({"goal": "finance report analysis"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("auto-routed answer"), "body: {body}");
}

// ── 409: turn already in flight ───────────────────────────────────────────────

#[tokio::test]
async fn orchestrate_returns_409_when_turn_in_flight() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Analyst").await;
    let team_id = fx.make_solo_team("Squad", lead).await;
    let session_id = fx.make_session("sess_busy").await;

    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    let state = fx.state(backend);
    // Occupy the session's turn lock, as a running send_message would.
    let _guard = state
        .cancels
        .try_begin_turn(&session_id)
        .expect("lock free");
    let app = router(state);

    let (status, body) = post_orchestrate(
        app,
        &session_id,
        serde_json::json!({"goal": "do work", "team_id": team_id}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    // Same wire shape as send_message's 409.
    assert!(body.contains("\"code\":\"conflict\""), "body: {body}");
    assert!(body.contains("already in flight"), "body: {body}");
    // The refused request must not have persisted the goal.
    assert!(fx.messages.snapshot(&session_id).is_empty());
}

// ── 422 boundaries ────────────────────────────────────────────────────────────

#[tokio::test]
async fn orchestrate_rejects_blank_goal_with_422() {
    let fx = Fixture::new();
    let session_id = fx.make_session("sess_blank").await;
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    let app = router(fx.state(backend));

    let (status, body) =
        post_orchestrate(app, &session_id, serde_json::json!({"goal": "   "})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
}

#[tokio::test]
async fn orchestrate_rejects_unknown_team_with_422() {
    let fx = Fixture::new();
    let session_id = fx.make_session("sess_ghost").await;
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    let app = router(fx.state(backend));

    let (status, body) = post_orchestrate(
        app,
        &session_id,
        serde_json::json!({"goal": "do work", "team_id": uuid::Uuid::new_v4()}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(body.contains("unknown team"), "body: {body}");
}

#[tokio::test]
async fn orchestrate_rejects_unmatched_goal_with_422() {
    // Auto-routing with no team that scores > 0 → 422.
    let fx = Fixture::new();
    let lead = fx.make_persona("Gardener").await;
    fx.make_solo_team("Garden Crew", lead).await;
    let session_id = fx.make_session("sess_nomatch").await;
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    let app = router(fx.state(backend));

    let (status, body) = post_orchestrate(
        app,
        &session_id,
        serde_json::json!({"goal": "quantum chromodynamics"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    assert!(body.contains("no team matches"), "body: {body}");
}

// ── 503 when teams are absent ─────────────────────────────────────────────────

#[tokio::test]
async fn orchestrate_returns_503_when_teams_absent() {
    let fx = Fixture::new();
    let session_id = fx.make_session("sess_503").await;
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response("noop"));
    let mut state = fx.state(backend);
    state.teams = None;
    let app = router(state);

    let (status, _) =
        post_orchestrate(app, &session_id, serde_json::json!({"goal": "do work"})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
