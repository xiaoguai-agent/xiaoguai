//! `GET /v1/mcp/servers` listing — globals for anonymous callers,
//! globals + tenant rows when claims are present.

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
use xiaoguai_types::{McpServer, McpServerInstanceId, McpTransport, TenantId};

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
    async fn create(&self, _tenant: Option<&str>, s: &McpServer) -> RepoResult<()> {
        self.rows.lock().push(s.clone());
        Ok(())
    }
    async fn find_by_id(&self, _tenant: Option<&str>, id: &str) -> RepoResult<Option<McpServer>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .find(|s| s.id.as_str() == id)
            .cloned())
    }
    async fn list_global(&self) -> RepoResult<Vec<McpServer>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .filter(|s| s.tenant_id.is_none())
            .cloned()
            .collect())
    }
    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<McpServer>> {
        Ok(self
            .rows
            .lock()
            .iter()
            .filter(|s| {
                s.tenant_id.is_none() || s.tenant_id.as_ref().map(AsRef::as_ref) == Some(tenant_id)
            })
            .cloned()
            .collect())
    }
    async fn delete(&self, _tenant: Option<&str>, _id: &str) -> RepoResult<()> {
        Ok(())
    }
}

fn dummy_server(name: &str, tenant: Option<&str>) -> McpServer {
    let now = chrono::Utc::now();
    McpServer {
        id: McpServerInstanceId::new(),
        tenant_id: tenant.map(|t| TenantId::from(t.to_string())),
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
    }
}

async fn body_arr(body: Body) -> Vec<Value> {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn anonymous_caller_sees_only_globals() {
    let mcp = InMemoryMcpRepo::arc();
    mcp.push(dummy_server("global-a", None));
    mcp.push(dummy_server("tenant-only", Some("ten_a")));
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
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["name"], "global-a");
}

#[tokio::test]
async fn authed_caller_sees_globals_plus_their_tenant() {
    let mcp = InMemoryMcpRepo::arc();
    mcp.push(dummy_server("global-a", None));
    mcp.push(dummy_server("tenant-only", Some("ten_a")));
    mcp.push(dummy_server("other-tenant", Some("ten_b")));

    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "alice".into(),
            tenant_id: "ten_a".into(),
            roles: vec![],
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
    assert!(names.contains(&"global-a"));
    assert!(names.contains(&"tenant-only"));
    assert!(!names.contains(&"other-tenant"));
}
