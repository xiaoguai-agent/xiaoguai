//! `StdioMcpClient` driven by our hand-rolled `mock-mcp-server` fixture.
//!
//! Cargo sets the `CARGO_BIN_EXE_mock-mcp-server` env var to the absolute
//! path of the fixture binary when compiling this integration test.

use serde_json::json;
use xiaoguai_mcp::{McpClient, StdioMcpClient};

fn fixture_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mock-mcp-server")
}

#[tokio::test]
async fn list_tools_and_call_round_trip() {
    let envs: [(&str, &str); 0] = [];
    let client = StdioMcpClient::spawn(fixture_bin(), &[], &envs)
        .await
        .expect("spawn");

    let info = client.initialize().await.expect("init");
    assert_eq!(info.name, "mock");
    assert_eq!(info.version, "0.1.0");

    let tools = client.list_tools().await.expect("list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert!(
        tools[0].input_schema.get("properties").is_some(),
        "schema: {:?}",
        tools[0].input_schema
    );

    let res = client
        .call_tool("echo", json!({"msg": "world"}))
        .await
        .expect("call");
    assert!(!res.is_error);
    assert_eq!(res.text, "echo: world");
    assert_eq!(res.blocks.len(), 1);

    client.shutdown().await.expect("shutdown");
}
