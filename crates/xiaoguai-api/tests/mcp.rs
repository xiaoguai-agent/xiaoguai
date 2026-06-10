//! `GET /v1/mcp/servers` listing — single-owner returns all servers.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use parking_lot::Mutex;
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{
    auth::{Claims, StubValidator, TokenValidator},
    router, AppState, CancelRegistry,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_storage::repositories::{McpServerRepository, RepoResult};
use xiaoguai_types::{McpServer, McpServerInstanceId, McpTransport};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

#[derive(Default)]
struct InMemoryMcpRepo {
    rows: Mutex<Vec<McpServer>>,
}

impl InMemoryMcpRepo {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    fn push(&self, s: McpServer) {
        self.rows.lock().push(s);
    }
}

#[async_trait]
impl McpServerRepository for InMemoryMcpRepo {
    async fn create(&self, s: &McpServer) -> RepoResult<()> {
        self.rows.lock().push(s.clone());
        Ok(())
    }
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .find(|s| s.id.as_str() == id)
            .cloned())
    }
    async fn list(&self) -> RepoResult<Vec<McpServer>> {
        Ok(self.rows.lock().iter().cloned().collect())
    }
    async fn delete(&self, _id: &str) -> RepoResult<()> {
        Ok(())
    }
}

fn dummy_server(name: &str, _tenant: Option<&str>) -> McpServer {
    let now = chrono::Utc::now();
    McpServer {
        id: McpServerInstanceId::new(),
        name: name.to_string(),
        version: "1.0.0".into(),
        transport: McpTransport::Stdio,
        command: Some("/bin/true".into()),
        args: vec![],
        env_keys: vec![],
        endpoint: None,
        enabled: true,
        created_at: now,
        updated_at: now,
    }
}

fn build_state(auth: Option<Arc<dyn TokenValidator>>, mcp: Arc<InMemoryMcpRepo>) -> AppState {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: Some(mcp),
        auth,
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
        personas: None,
        watchers: None,
        loops: None,
        teams: None,
        team_audit: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
    }
}

async fn body_arr(body: Body) -> Vec<Value> {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn anonymous_caller_sees_all_servers() {
    let mcp = InMemoryMcpRepo::arc();
    mcp.push(dummy_server("server-a", None));
    mcp.push(dummy_server("server-b", None));
    let app = router(build_state(None, mcp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/mcp/servers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    assert_eq!(v.len(), 2);
}

#[tokio::test]
async fn authed_caller_sees_all_servers() {
    let mcp = InMemoryMcpRepo::arc();
    mcp.push(dummy_server("server-a", None));
    mcp.push(dummy_server("server-b", None));

    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "alice".into(),
        },
    });
    let app = router(build_state(Some(validator), mcp));

    let req = Request::builder()
        .uri("/v1/mcp/servers")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_arr(resp.into_body()).await;
    let names: Vec<&str> = v.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"server-a"));
    assert!(names.contains(&"server-b"));
}
