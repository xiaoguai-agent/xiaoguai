//! Integration test for `GET /v1/admin/me/scopes` (sprint-10b S10b-6).
//!
//! Exercises:
//!   * Fail-open: when `authz = None`, the endpoint returns the full
//!     scope vocabulary (dev-mode behaviour, matches per-route layer).
//!   * Casbin-resolved: when `authz = Some`, the response contains only
//!     scopes the bearer's roles can satisfy. `tenant_admin` gets the
//!     persona / memory / watcher / audit scopes; `system_admin` gets
//!     everything; an empty-role token gets `[]`.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::Authz;
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

#[derive(Debug, Deserialize)]
struct ScopesBody {
    scopes: Vec<String>,
}

async fn build_state(roles: Vec<&str>, wire_authz: bool) -> AppState {
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "u".into(),
            tenant_id: "ten_a".into(),
            roles: roles.into_iter().map(str::to_string).collect(),
        },
    });
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let authz = if wire_authz {
        Some(Arc::new(Authz::new_default().await.expect("authz")))
    } else {
        None
    };
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: Some(validator),
        authz,
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
        decision_registry: std::sync::Arc::new(xiaoguai_api::hotl::decision_registry::DecisionRegistry::new()),
    }
}

async fn fetch_scopes(state: AppState) -> (StatusCode, Vec<String>) {
    let app = router(state);
    let req = Request::builder()
        .uri("/v1/admin/me/scopes")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    if status != StatusCode::OK {
        return (status, vec![]);
    }
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: ScopesBody = serde_json::from_slice(&bytes).unwrap();
    (status, body.scopes)
}

#[tokio::test]
async fn fail_open_returns_full_vocabulary_when_no_authz() {
    let (status, scopes) = fetch_scopes(build_state(vec!["anything"], false).await).await;
    assert_eq!(status, StatusCode::OK);
    // The exact list is implementation-defined but must include the
    // scopes the admin-ui needs to gate buttons; assert on a handful.
    assert!(scopes.contains(&"personas.write".to_string()));
    assert!(scopes.contains(&"skill.approve".to_string()));
    assert!(scopes.contains(&"audit.export".to_string()));
}

#[tokio::test]
async fn tenant_admin_gets_persona_and_memory_scopes() {
    let (status, scopes) = fetch_scopes(build_state(vec!["tenant_admin"], true).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        scopes.contains(&"personas.write".to_string()),
        "tenant_admin policy line grants /personas/* write → personas.write must be present; got {scopes:?}"
    );
    assert!(scopes.contains(&"personas.read".to_string()));
    assert!(scopes.contains(&"memories.read".to_string()));
    assert!(scopes.contains(&"watchers.read".to_string()));
    // tenant_admin does NOT have the skill proposals policy line.
    assert!(
        !scopes.contains(&"skill.approve".to_string()),
        "tenant_admin must not get skill.approve (no policy rule); got {scopes:?}"
    );
}

#[tokio::test]
async fn system_admin_gets_every_scope() {
    let (status, scopes) = fetch_scopes(build_state(vec!["system_admin"], true).await).await;
    assert_eq!(status, StatusCode::OK);
    // system_admin's grant is (*, *, *, *) so every scope in the map
    // resolves.
    for required in [
        "personas.read",
        "personas.write",
        "personas.delete",
        "memories.read",
        "memories.write",
        "watchers.read",
        "skill.approve",
        "audit.export",
        "audit.read",
    ] {
        assert!(
            scopes.contains(&required.to_string()),
            "system_admin must get {required}; got {scopes:?}"
        );
    }
}

#[tokio::test]
async fn member_gets_only_read_scopes() {
    let (status, scopes) = fetch_scopes(build_state(vec!["member"], true).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(scopes.contains(&"personas.read".to_string()));
    assert!(scopes.contains(&"memories.read".to_string()));
    assert!(
        !scopes.contains(&"personas.write".to_string()),
        "member should not get personas.write; got {scopes:?}"
    );
    assert!(
        !scopes.contains(&"audit.export".to_string()),
        "member should not get audit.export; got {scopes:?}"
    );
}
