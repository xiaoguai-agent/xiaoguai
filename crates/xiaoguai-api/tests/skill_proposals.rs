//! Tier-2 D.1 — Integration coverage for `/v1/skills/proposals/*`.
//!
//! Exercises the list / approve / reject endpoints through the real
//! axum router + `AppState`, with the in-memory fixtures from
//! `xiaoguai_tasks::skill_author`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_tasks::skill_author::{
    AllowAllSkillGate, InMemoryAuditSink, InMemorySkillProposalRepository, InMemoryTenantSettings,
    ProposalRow, ProposalStatus, SkillAuditSink, SkillAuthorGate, SkillManifest,
    SkillProposalRepository, TenantSettingsReader,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

struct Fixture {
    state: AppState,
    repo: Arc<InMemorySkillProposalRepository>,
    audit: Arc<InMemoryAuditSink>,
    tmp: tempfile::TempDir,
}

fn tenant_uuid() -> &'static str {
    "00000000-0000-0000-0000-000000000099"
}

fn good_manifest() -> SkillManifest {
    SkillManifest {
        name: "ar-collector".into(),
        description: "Collect AR invoices".into(),
        version: "0.1.0".into(),
        system_prompt: "You collect AR".into(),
        tool_allowlist: vec!["search".into()],
    }
}

fn build_fixture() -> Fixture {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let repo = InMemorySkillProposalRepository::new();
    let repo_arc: Arc<dyn SkillProposalRepository> = repo.clone();
    let settings = InMemoryTenantSettings::new();
    settings.allow(tenant_uuid());
    let settings_arc: Arc<dyn TenantSettingsReader> = settings;
    let gate_arc: Arc<dyn SkillAuthorGate> = Arc::new(AllowAllSkillGate);
    let audit = InMemoryAuditSink::new();
    let audit_arc: Arc<dyn SkillAuditSink> = audit.clone();
    let tmp = tempfile::tempdir().unwrap();

    let state = AppState {
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
        skill_proposals: Some(repo_arc),
        tenant_settings: Some(settings_arc),
        skill_author_gate: Some(gate_arc),
        skill_audit: Some(audit_arc),
        skills_dir: tmp.path().to_path_buf(),
        personas: None,
    };
    Fixture {
        state,
        repo,
        audit,
        tmp,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn seed_pending(fx: &Fixture, name: &str) -> ProposalRow {
    let mut m = good_manifest();
    m.name = name.into();
    let row = ProposalRow {
        id: format!("prop-{name}"),
        tenant_id: tenant_uuid().to_string(),
        proposed_by: "agent-1".into(),
        manifest: m,
        status: ProposalStatus::Pending,
        reason: None,
        created_at: chrono::Utc::now(),
        decided_at: None,
        decided_by: None,
    };
    fx.repo.insert(row.clone()).await.unwrap()
}

#[tokio::test]
async fn list_proposals_returns_seeded_rows_newest_first() {
    let fx = build_fixture();
    seed_pending(&fx, "skill-a").await;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    seed_pending(&fx, "skill-b").await;

    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/v1/skills/proposals?tenant_id={}",
            tenant_uuid()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["manifest"]["name"], "skill-b");
    assert_eq!(arr[1]["manifest"]["name"], "skill-a");
}

#[tokio::test]
async fn list_proposals_filters_by_status() {
    let fx = build_fixture();
    seed_pending(&fx, "skill-pending").await;
    let approved = seed_pending(&fx, "skill-installed").await;
    fx.repo
        .set_status(&approved.id, ProposalStatus::Installed, "admin", None)
        .await
        .unwrap();

    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/v1/skills/proposals?tenant_id={}&status=pending",
            tenant_uuid()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["manifest"]["name"], "skill-pending");
}

#[tokio::test]
async fn approve_flips_status_and_writes_yaml() {
    let fx = build_fixture();
    let row = seed_pending(&fx, "ar-collector").await;

    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/skills/proposals/{}/approve", row.id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"decided_by": "admin-1"})).unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["status"], "installed");
    assert_eq!(v["decided_by"], "admin-1");

    let yaml = fx.tmp.path().join("ar-collector-0.1.0.yaml");
    assert!(yaml.exists(), "yaml should land on disk");

    let actions: Vec<_> = fx
        .audit
        .entries()
        .iter()
        .map(|e| e.action.clone())
        .collect();
    assert_eq!(actions, vec!["skill.approve"]);
}

#[tokio::test]
async fn approve_returns_404_for_missing_proposal() {
    let fx = build_fixture();
    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/skills/proposals/no-such-id/approve")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"decided_by": "admin-1"})).unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reject_flips_status_with_reason() {
    let fx = build_fixture();
    let row = seed_pending(&fx, "ar-collector").await;

    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/skills/proposals/{}/reject", row.id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "decided_by": "admin-1",
                "reason": "tool_allowlist too broad",
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp.into_body()).await;
    assert_eq!(v["status"], "rejected");
    assert_eq!(v["reason"], "tool_allowlist too broad");
}

#[tokio::test]
async fn list_returns_503_when_proposals_unwired() {
    let mut fx = build_fixture();
    fx.state.skill_proposals = None;
    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/v1/skills/proposals?tenant_id={}",
            tenant_uuid()
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_returns_400_when_tenant_missing() {
    let fx = build_fixture();
    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::GET)
        .uri("/v1/skills/proposals")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Reads but doesn't assert; just makes sure the path resolution is what
/// we documented (avoids drift between SKILL.md and runtime).
#[tokio::test]
async fn approved_yaml_round_trips_back_to_the_proposed_manifest() {
    let fx = build_fixture();
    let row = seed_pending(&fx, "ar-collector").await;

    let app = router(fx.state.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/skills/proposals/{}/approve", row.id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"decided_by": "admin-1"})).unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let yaml_path: PathBuf = fx.tmp.path().join("ar-collector-0.1.0.yaml");
    let parsed: SkillManifest =
        serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
    assert_eq!(parsed, row.manifest);
}
