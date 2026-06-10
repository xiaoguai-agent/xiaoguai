//! Integration tests for `/v1/teams` + `/v1/sessions/:id/team` (T3.2).
//!
//! Boots the production router with in-memory team + persona repositories and
//! exercises: CRUD round-trip, the 503 fallback when `teams` is `None`,
//! member-existence validation at the boundary, the attach path (which must
//! ALSO attach the team's lead persona via `session_personas`), and the
//! best-effort `team.*` audit entries.

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
use xiaoguai_personas::{
    CreatePersonaRequest, InMemoryPersonaRepository, InMemoryTeamRepository, PersonaRepository,
    TeamRepository,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

struct Fixture {
    personas: Arc<dyn PersonaRepository>,
    teams: Arc<dyn TeamRepository>,
    audit: Arc<InMemoryHotlAuditSink>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            personas: Arc::new(InMemoryPersonaRepository::new()),
            teams: Arc::new(InMemoryTeamRepository::new()),
            audit: Arc::new(InMemoryHotlAuditSink::new()),
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
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
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
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

// ── 503 fallback ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn teams_routes_return_503_when_repo_absent() {
    let fx = Fixture::new();
    let mut state = fx.state();
    state.teams = None;
    let app = router(state);

    let (status, _) = send(app, "GET", "/v1/teams", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ── CRUD round-trip ───────────────────────────────────────────────────────────

#[tokio::test]
async fn team_crud_round_trip_with_audit() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Analyst").await;
    let worker = fx.make_persona("Worker").await;
    let app = router(fx.state());

    // Create.
    let (status, team) = send(
        app.clone(),
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Finance Squad",
            "description": "Reports.",
            "lead_persona_id": lead,
            "member_persona_ids": [lead, worker],
            "recommended_pack_slugs": ["office-tools"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {team}");
    let team_id = team["id"].as_str().expect("team id").to_string();

    // List + get.
    let (status, list) = send(app.clone(), "GET", "/v1/teams", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (status, fetched) = send(app.clone(), "GET", &format!("/v1/teams/{team_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["name"], "Finance Squad");

    // Update.
    let (status, updated) = send(
        app.clone(),
        "PATCH",
        &format!("/v1/teams/{team_id}"),
        Some(serde_json::json!({"description": "Annual reports."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["description"], "Annual reports.");

    // Archive.
    let (status, _) = send(app.clone(), "DELETE", &format!("/v1/teams/{team_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = send(app.clone(), "GET", "/v1/teams", None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    // Audit entries were emitted best-effort.
    let actions: Vec<String> = fx.audit.snapshot().into_iter().map(|e| e.action).collect();
    assert_eq!(actions, vec!["team.create", "team.update", "team.archive"]);
}

// ── Boundary validation: members must exist and be active ────────────────────

#[tokio::test]
async fn create_team_rejects_unknown_member_persona() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Analyst").await;
    let ghost = uuid::Uuid::new_v4();
    let app = router(fx.state());

    let (status, body) = send(
        app,
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Ghost Team",
            "lead_persona_id": lead,
            "member_persona_ids": [lead, ghost],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn create_team_rejects_archived_member_persona() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Analyst").await;
    let retired = fx.make_persona("Retired").await;
    fx.personas.archive_persona(retired).await.unwrap();
    let app = router(fx.state());

    let (status, _) = send(
        app,
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Stale Team",
            "lead_persona_id": lead,
            "member_persona_ids": [lead, retired],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── Session attach: must also attach the lead persona ────────────────────────

#[tokio::test]
async fn attach_team_attaches_lead_persona_and_audits() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Analyst").await;
    let worker = fx.make_persona("Worker").await;
    let app = router(fx.state());

    let (_, team) = send(
        app.clone(),
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Squad",
            "lead_persona_id": lead,
            "member_persona_ids": [lead, worker],
        })),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();

    // Attach to a session.
    let (status, att) = send(
        app.clone(),
        "PUT",
        "/v1/sessions/sess_1/team",
        Some(serde_json::json!({"team_id": team_id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "attach failed: {att}");

    // The team is attached…
    let (status, active) = send(app.clone(), "GET", "/v1/sessions/sess_1/team", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["id"].as_str().unwrap(), team_id);

    // …and so is its LEAD persona (zero ReAct-loop changes — the session
    // runs with the lead until T4).
    let lead_active = fx
        .personas
        .get_session_persona("sess_1")
        .await
        .unwrap()
        .expect("lead persona must be attached alongside the team");
    assert_eq!(lead_active.id, lead);

    // Audit trail: create + attach.
    let actions: Vec<String> = fx.audit.snapshot().into_iter().map(|e| e.action).collect();
    assert_eq!(actions, vec!["team.create", "team.attach"]);

    // Detach clears the team but leaves the persona (operator may still
    // want the expert active; detaching the persona is the explicit
    // DELETE /v1/sessions/:id/persona).
    let (status, _) = send(app.clone(), "DELETE", "/v1/sessions/sess_1/team", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(app.clone(), "GET", "/v1/sessions/sess_1/team", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn attach_unknown_team_returns_404() {
    let fx = Fixture::new();
    let app = router(fx.state());
    let (status, _) = send(
        app,
        "PUT",
        "/v1/sessions/sess_1/team",
        Some(serde_json::json!({"team_id": uuid::Uuid::new_v4()})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── T7.1 glossary through the routes (set / clear / cap → 400) ────────────────

#[tokio::test]
async fn team_glossary_set_clear_and_cap_through_routes() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Glossarist").await;
    let app = router(fx.state());

    // Create with a glossary.
    let (status, created) = send(
        app.clone(),
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Glossary Squad",
            "lead_persona_id": lead,
            "member_persona_ids": [lead],
            "glossary_md": "MRR = monthly recurring revenue",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {created}");
    assert_eq!(created["glossary_md"], "MRR = monthly recurring revenue");
    let id = created["id"].as_str().unwrap();

    // PATCH a new value.
    let (status, updated) = send(
        app.clone(),
        "PATCH",
        &format!("/v1/teams/{id}"),
        Some(serde_json::json!({"glossary_md": "ARR only"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["glossary_md"], "ARR only");

    // PATCH of an unrelated field keeps the glossary.
    let (status, updated) = send(
        app.clone(),
        "PATCH",
        &format!("/v1/teams/{id}"),
        Some(serde_json::json!({"description": "desc"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["glossary_md"], "ARR only");

    // A blank value clears (normalises to null).
    let (status, cleared) = send(
        app.clone(),
        "PATCH",
        &format!("/v1/teams/{id}"),
        Some(serde_json::json!({"glossary_md": "  "})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(cleared["glossary_md"].is_null());

    // Over the 16 KiB cap → 400 with a clear message (no truncation).
    let oversized = "x".repeat(16_385);
    let (status, err) = send(
        app.clone(),
        "PATCH",
        &format!("/v1/teams/{id}"),
        Some(serde_json::json!({"glossary_md": oversized})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"].as_str().unwrap().contains("16384"),
        "error names the cap: {err}"
    );
    // Value untouched after the rejected update.
    let (_, fetched) = send(app.clone(), "GET", &format!("/v1/teams/{id}"), None).await;
    assert!(fetched["glossary_md"].is_null());
}

#[tokio::test]
async fn create_team_over_glossary_cap_returns_400() {
    let fx = Fixture::new();
    let lead = fx.make_persona("Capped").await;
    let app = router(fx.state());

    let (status, err) = send(
        app,
        "POST",
        "/v1/teams",
        Some(serde_json::json!({
            "name": "Too Big",
            "lead_persona_id": lead,
            "member_persona_ids": [lead],
            "glossary_md": "x".repeat(16_385),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "got: {err}");
    assert!(err["error"].as_str().unwrap().contains("glossary_md"));
}
