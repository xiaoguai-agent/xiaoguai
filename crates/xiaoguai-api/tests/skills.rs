//! Integration coverage for v1.2.28: `/v1/skills/*` endpoints.
//!
//! Exercises catalog listing, install round-trip, uninstall, duplicate
//! install (409 Conflict), unknown slug (404), and the 503 unwired path —
//! all through the real axum router + `AppState`.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::skills::{InMemorySkillPackRepository, SkillPackRepository};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(skill_packs: Option<Arc<dyn SkillPackRepository>>) -> AppState {
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
        skill_packs,
        memory_store: None,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers: None,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "test helper — caller owns the Value"
)]
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ── catalog ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn catalog_lists_all_nine_packs() {
    let app = router(build_state(None)); // catalog endpoint never needs the repo
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/skills/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let packs = body["packs"].as_array().unwrap();
    assert_eq!(packs.len(), 9, "catalog must ship exactly 9 packs");

    let slugs: Vec<&str> = packs.iter().map(|p| p["slug"].as_str().unwrap()).collect();
    for expected in &[
        "ar-collections",
        "incident-triage",
        "pr-review",
        "hr-onboarding",
        "rag-legal",
        "rag-finance",
        "rag-hr",
        "devops-oncall",
        "sales-qualification",
    ] {
        assert!(slugs.contains(expected), "missing slug: {expected}");
    }
}

// ── installed — 503 when repo not wired ─────────────────────────────────────

#[tokio::test]
async fn installed_503_when_repo_not_wired() {
    let app = router(build_state(None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/skills/installed?tenant=t1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── install round-trip ───────────────────────────────────────────────────────

#[tokio::test]
async fn install_then_list_then_uninstall() {
    let repo = InMemorySkillPackRepository::new();
    let app = router(build_state(Some(repo as Arc<dyn SkillPackRepository>)));

    // Install
    let install_resp = app
        .clone()
        .oneshot(post_json(
            "/v1/skills/install",
            serde_json::json!({
                "tenant_id": "t1",
                "pack_slug": "rag-hr",
                "config": { "top_k": 10 }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(install_resp.status(), StatusCode::OK);
    let installed = body_json(install_resp.into_body()).await;
    let id = installed["id"].as_str().unwrap().to_string();
    assert_eq!(installed["pack_slug"], "rag-hr");
    assert_eq!(installed["version"], "1.0.0");

    // List — must see the installed row.
    let after_install_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/skills/installed?tenant=t1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_install_resp.status(), StatusCode::OK);
    let listed = body_json(after_install_resp.into_body()).await;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);
    assert_eq!(arr[0]["config"]["top_k"], 10);

    // Uninstall
    let del_resp = app
        .clone()
        .oneshot(delete(&format!("/v1/skills/install/{id}")))
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);
    let del_body = body_json(del_resp.into_body()).await;
    assert_eq!(del_body["deleted"], id);

    // List again — must be empty.
    let after_uninstall_resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/skills/installed?tenant=t1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_uninstall_resp.status(), StatusCode::OK);
    let after_uninstall = body_json(after_uninstall_resp.into_body()).await;
    assert!(after_uninstall.as_array().unwrap().is_empty());
}

// ── duplicate install returns 409 ───────────────────────────────────────────

#[tokio::test]
async fn duplicate_install_returns_conflict() {
    let repo = InMemorySkillPackRepository::new();
    let app = router(build_state(Some(repo as Arc<dyn SkillPackRepository>)));

    let r1 = app
        .clone()
        .oneshot(post_json(
            "/v1/skills/install",
            serde_json::json!({"tenant_id": "t1", "pack_slug": "pr-review"}),
        ))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    let r2 = app
        .oneshot(post_json(
            "/v1/skills/install",
            serde_json::json!({"tenant_id": "t1", "pack_slug": "pr-review"}),
        ))
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::CONFLICT);
}

// ── unknown slug returns 404 ─────────────────────────────────────────────────

#[tokio::test]
async fn install_unknown_slug_returns_not_found() {
    let repo = InMemorySkillPackRepository::new();
    let app = router(build_state(Some(repo as Arc<dyn SkillPackRepository>)));

    let resp = app
        .oneshot(post_json(
            "/v1/skills/install",
            serde_json::json!({"tenant_id": "t1", "pack_slug": "no-such-pack"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── list scopes by tenant ────────────────────────────────────────────────────

#[tokio::test]
async fn list_installed_scopes_by_tenant() {
    let repo = InMemorySkillPackRepository::new();
    let app = router(build_state(Some(
        repo.clone() as Arc<dyn SkillPackRepository>
    )));

    for (tenant, slug) in &[("t1", "rag-legal"), ("t1", "rag-finance"), ("t2", "rag-hr")] {
        app.clone()
            .oneshot(post_json(
                "/v1/skills/install",
                serde_json::json!({"tenant_id": tenant, "pack_slug": slug}),
            ))
            .await
            .unwrap();
    }

    let t1_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/skills/installed?tenant=t1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let t1 = body_json(t1_resp.into_body()).await;
    assert_eq!(t1.as_array().unwrap().len(), 2);

    let t2_resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/skills/installed?tenant=t2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let t2 = body_json(t2_resp.into_body()).await;
    assert_eq!(t2.as_array().unwrap().len(), 1);
}
