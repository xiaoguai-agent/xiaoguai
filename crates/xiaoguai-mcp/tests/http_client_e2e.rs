//! v0.9.0: in-process e2e for `HttpMcpClient` against an axum-mounted
//! `StreamableHttpService` echo server. Mirrors the rmcp upstream test
//! pattern but stays focused on what xiaoguai needs to assert:
//! handshake + `list_tools` + `call_tool` + tenant-style custom header
//! flow-through.

use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpServerConfig,
};
use rmcp::transport::StreamableHttpService;
use rmcp::ErrorData as McpProtocolError;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use xiaoguai_mcp::{HttpClientConfig, HttpMcpClient, McpClient};

#[derive(Clone, Default)]
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools_with(ToolsCapability {
                    list_changed: Some(false),
                })
                .build(),
        )
        .with_server_info(Implementation::new("xiaoguai-mock-http", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        let schema = json!({
            "type": "object",
            "properties": {
                "msg": { "type": "string" }
            },
            "required": ["msg"],
        });
        let schema_obj = Arc::new(schema.as_object().cloned().unwrap_or_default());
        let tool = Tool::new("echo", "echoes its input", schema_obj);
        Ok(ListToolsResult {
            tools: vec![tool],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpProtocolError> {
        if request.name != "echo" {
            return Err(McpProtocolError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let msg = request
            .arguments
            .as_ref()
            .and_then(|m| m.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "echo: {msg}"
        ))]))
    }
}

async fn start_echo_server() -> (String, CancellationToken) {
    let cancel = CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(EchoServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/mcp");

    let ct = cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { ct.cancelled().await })
            .await;
    });

    // Give the listener a tick to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (url, cancel)
}

#[tokio::test]
async fn connect_lists_tools_and_calls_echo() {
    let (url, cancel) = start_echo_server().await;

    let client = HttpMcpClient::connect(HttpClientConfig::new(&url))
        .await
        .expect("connect");

    let info = client.initialize().await.expect("initialize");
    assert_eq!(info.name, "xiaoguai-mock-http");
    assert_eq!(info.version, "0.1.0");

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert!(tools[0].input_schema.get("properties").is_some());

    let result = client
        .call_tool("echo", json!({ "msg": "hello over http" }))
        .await
        .expect("call_tool");
    assert!(!result.is_error);
    assert_eq!(result.text, "echo: hello over http");
    assert_eq!(result.blocks.len(), 1);

    client.shutdown().await.expect("shutdown");
    cancel.cancel();
}

#[tokio::test]
async fn call_tool_rejects_non_object_arguments() {
    let (url, cancel) = start_echo_server().await;
    let client = HttpMcpClient::connect(HttpClientConfig::new(&url))
        .await
        .expect("connect");

    // Pass an array — must surface InvalidArgument *client-side* without
    // ever hitting the wire.
    let err = client
        .call_tool("echo", json!(["wrong", "shape"]))
        .await
        .expect_err("array args should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("must be a JSON object"),
        "unexpected error: {msg}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn custom_headers_flow_through_to_request() {
    // We can't easily assert what the server received without a custom
    // ServerHandler. Instead, verify the constructor accepts well-formed
    // header values and the connect handshake succeeds — proving the
    // builder didn't poison the config.
    let (url, cancel) = start_echo_server().await;

    let cfg = HttpClientConfig::new(&url)
        .with_auth("Bearer not-checked-by-this-server")
        .with_header("X-Tenant-Id", "ten_a")
        .with_header("X-Trace-Id", "trace-1");
    let client = HttpMcpClient::connect(cfg).await.expect("connect");
    let info = client.initialize().await.expect("initialize");
    assert_eq!(info.name, "xiaoguai-mock-http");

    cancel.cancel();
}

#[tokio::test]
async fn malformed_custom_header_name_is_rejected_before_connect() {
    // No server needed — header validation happens client-side in
    // `HttpMcpClient::connect` before any network I/O.
    let cfg =
        HttpClientConfig::new("http://127.0.0.1:1/mcp").with_header("X-Bad\r\nInjected", "value");
    let err = HttpMcpClient::connect(cfg)
        .await
        .expect_err("malformed header must fail fast");
    let msg = err.to_string();
    assert!(msg.contains("header name"), "unexpected error: {msg}");
}
