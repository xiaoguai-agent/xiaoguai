//! v0.9.1: end-to-end coverage for publishing xiaoguai's `Toolbox` as
//! an MCP server. Spins up the full router (with `mcp_publish_enabled =
//! true`), points an `HttpMcpClient` at `/v1/mcp/serve`, and asserts
//! handshake + `list_tools` + `call_tool` round-trip — exercising the
//! `Toolbox → ServerHandler → rmcp transport → reqwest client` path
//! exactly as an external agent would.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{serve_with_state, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_mcp::{
    ContentBlock, HttpClientConfig, HttpMcpClient, McpClient, McpResult,
    ServerInfo as McpServerInfo, ToolDescriptor, ToolResult,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

/// Tiny stub MCP backend whose only behaviour is to echo `{ msg }`
/// back as text. Used to seed the `Toolbox` so the published MCP
/// server actually has something to dispatch.
struct EchoBackend;

#[async_trait]
impl McpClient for EchoBackend {
    async fn initialize(&self) -> McpResult<McpServerInfo> {
        Ok(McpServerInfo {
            name: "echo-backend".into(),
            version: "0".into(),
        })
    }
    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        Ok(ToolResult {
            text: format!("{name}: {msg}"),
            blocks: vec![ContentBlock::Text {
                text: format!("{name}: {msg}"),
            }],
            is_error: false,
        })
    }
    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

fn build_state_with_toolbox(toolbox: Toolbox, publish: bool) -> AppState {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(toolbox),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        authz: None,
        tenants: None,
        rate_limiter: None,
        audit: None,
        audit_verifier: None,
        mcp_publish_enabled: publish,
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
    }
}

fn fixture_toolbox() -> Toolbox {
    let client: Arc<dyn McpClient> = Arc::new(EchoBackend);
    let descriptors = vec![
        ToolDescriptor {
            name: "echo".into(),
            description: Some("echo back the msg argument".into()),
            input_schema: json!({
                "type": "object",
                "properties": { "msg": { "type": "string" } },
                "required": ["msg"]
            }),
        },
        ToolDescriptor {
            name: "ping".into(),
            description: Some("returns pong-formatted text".into()),
            input_schema: json!({
                "type": "object",
                "properties": { "msg": { "type": "string" } }
            }),
        },
    ];
    Toolbox::from_server(client, descriptors).expect("toolbox")
}

async fn spawn(state: AppState) -> String {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (bound, fut) = serve_with_state(addr, state).await.expect("bind");
    tokio::spawn(fut);
    // Tick to let the listener start accepting.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://{bound}/v1/mcp/serve")
}

#[tokio::test]
async fn published_server_exposes_toolbox_tools() {
    let state = build_state_with_toolbox(fixture_toolbox(), true);
    let url = spawn(state).await;

    let client = HttpMcpClient::connect(HttpClientConfig::new(&url))
        .await
        .expect("connect");

    let info = client.initialize().await.expect("initialize");
    assert_eq!(info.name, "xiaoguai");

    let mut tools = client.list_tools().await.expect("list_tools");
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[1].name, "ping");
    assert!(tools[0].input_schema.get("properties").is_some());
}

#[tokio::test]
async fn published_call_tool_round_trips_through_underlying_client() {
    let state = build_state_with_toolbox(fixture_toolbox(), true);
    let url = spawn(state).await;
    let client = HttpMcpClient::connect(HttpClientConfig::new(&url))
        .await
        .expect("connect");

    let out = client
        .call_tool("echo", json!({ "msg": "hi from external agent" }))
        .await
        .expect("call_tool");
    assert!(!out.is_error);
    assert_eq!(out.text, "echo: hi from external agent");
    assert_eq!(out.blocks.len(), 1);
}

#[tokio::test]
async fn unknown_tool_returns_error_response() {
    let state = build_state_with_toolbox(fixture_toolbox(), true);
    let url = spawn(state).await;
    let client = HttpMcpClient::connect(HttpClientConfig::new(&url))
        .await
        .expect("connect");

    let err = client
        .call_tool("does_not_exist", json!({}))
        .await
        .expect_err("unknown tool should error");
    let msg = err.to_string();
    assert!(msg.contains("unknown tool"), "unexpected error: {msg}");
}

#[tokio::test]
async fn publishing_disabled_means_endpoint_404s() {
    // Same setup, just with publishing turned off — the route should
    // not be mounted, so connect() / initialize() must fail.
    let state = build_state_with_toolbox(fixture_toolbox(), false);
    let url = spawn(state).await;

    // We deliberately keep a tight timeout: the route doesn't exist, so
    // the initialize handshake should fail fast (not retry forever).
    let fut = HttpMcpClient::connect(HttpClientConfig::new(&url));
    let res = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await;
    let inner =
        res.expect("connect to disabled endpoint hung past 5s — rmcp should fail fast on 404");
    assert!(
        inner.is_err(),
        "connect succeeded against a disabled publish endpoint"
    );
}
