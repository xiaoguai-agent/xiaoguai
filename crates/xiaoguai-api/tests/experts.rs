//! Integration tests for `POST /v1/experts/suggest` (T3.3).
//!
//! Deterministic, offline expert suggestion: tokenize the goal, score active
//! personas and teams by keyword overlap (name weighted over prompt/description),
//! return a ranked list. No LLM call (owner decision ②A).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_personas::teams::model::CreateTeamRequest;
use xiaoguai_personas::{
    CreatePersonaRequest, InMemoryPersonaRepository, InMemoryTeamRepository, PersonaRepository,
    TeamRepository,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

struct Fixture {
    personas: Arc<dyn PersonaRepository>,
    teams: Arc<dyn TeamRepository>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            personas: Arc::new(InMemoryPersonaRepository::new()),
            teams: Arc::new(InMemoryTeamRepository::new()),
        }
    }

    fn state(&self) -> AppState {
        let backend: Arc<dyn LlmBackend> =
            Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
        AppState {
            sessions: InMemorySessionRepo::arc(),
            messages: InMemoryMessageRepo::arc(),
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
            team_audit: None,
            watchers: None,
            loops: None,
            decision_registry: std::sync::Arc::new(
                xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
            ),
            pack_rescanner: None,
        }
    }

    async fn make_persona(&self, name: &str, prompt: &str) -> uuid::Uuid {
        self.personas
            .create(&CreatePersonaRequest {
                name: name.to_string(),
                system_prompt: prompt.to_string(),
                default_model: None,
                tool_allowlist: None,
                escalation_tier: None,
            })
            .await
            .expect("create persona")
            .id
    }
}

async fn suggest(app: axum::Router, goal: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/experts/suggest")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({ "goal": goal }).to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn suggest_returns_503_when_personas_absent() {
    let fx = Fixture::new();
    let mut state = fx.state();
    state.personas = None;
    let app = router(state);
    let (status, _) = suggest(app, "review my finance report").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn suggest_ranks_matching_persona_first() {
    let fx = Fixture::new();
    fx.make_persona(
        "Finance Analyst",
        "role/planner You analyse financial reports.",
    )
    .await;
    fx.make_persona(
        "Support Bot",
        "role/worker You answer customer support tickets.",
    )
    .await;
    let app = router(fx.state());

    let (status, body) = suggest(app, "analyse the quarterly finance report").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let suggestions = body["suggestions"].as_array().expect("array");
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0]["name"], "Finance Analyst");
    assert_eq!(suggestions[0]["kind"], "persona");
    assert!(suggestions[0]["score"].as_u64().unwrap() > 0);
    // The non-matching persona must not outrank the matching one; if present
    // at all it carries a strictly lower score.
    for s in &suggestions[1..] {
        assert!(s["score"].as_u64().unwrap() <= suggestions[0]["score"].as_u64().unwrap());
    }
}

#[tokio::test]
async fn suggest_includes_matching_team() {
    let fx = Fixture::new();
    let lead = fx
        .make_persona("Finance Analyst", "role/planner Financial analysis.")
        .await;
    fx.teams
        .create(&CreateTeamRequest {
            name: "Finance Squad".to_string(),
            description: "Quarterly finance reports end to end.".to_string(),
            lead_persona_id: lead,
            member_persona_ids: vec![lead],
            recommended_pack_slugs: vec![],
            glossary_md: None,
        })
        .await
        .unwrap();
    let app = router(fx.state());

    let (status, body) = suggest(app, "finance report").await;
    assert_eq!(status, StatusCode::OK);
    let kinds: Vec<&str> = body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"team"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"persona"), "kinds: {kinds:?}");
}

#[tokio::test]
async fn suggest_matches_cjk_goals() {
    let fx = Fixture::new();
    fx.make_persona("财务分析师", "role/planner 你负责财务报表分析。")
        .await;
    fx.make_persona("客服机器人", "role/worker 你回答客户问题。")
        .await;
    let app = router(fx.state());

    let (status, body) = suggest(app, "帮我做财务报表").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let suggestions = body["suggestions"].as_array().unwrap();
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0]["name"], "财务分析师");
}

#[tokio::test]
async fn suggest_with_no_match_returns_empty_list() {
    let fx = Fixture::new();
    fx.make_persona("Finance Analyst", "Financial analysis.")
        .await;
    let app = router(fx.state());

    let (status, body) = suggest(app, "xylophone juggling").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["suggestions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn suggest_rejects_blank_goal() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, _) = suggest(app, "   ").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
