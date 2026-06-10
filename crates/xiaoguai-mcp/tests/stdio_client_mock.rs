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

/// SEC-03: the host environment must NOT leak into MCP server children.
///
/// Probes via the fixture's `env_probe` tool, which reports whether a var is
/// visible inside the child. Uses `CARGO_MANIFEST_DIR` as the leak canary —
/// cargo/nextest always set it in the parent test process, it is not on the
/// spawn allowlist, and probing it avoids the process-global (and
/// thread-unsafe) `std::env::set_var`.
#[tokio::test]
async fn spawn_scrubs_host_env_from_child() {
    if std::env::var("CARGO_MANIFEST_DIR").is_err() {
        // Running the test binary outside cargo: no canary var to probe.
        return;
    }

    let envs = [("XG_EXPLICIT", "visible")];
    let client = StdioMcpClient::spawn(fixture_bin(), &[], &envs)
        .await
        .expect("spawn");

    // Parent-only var is scrubbed by env_clear.
    let leaked = client
        .call_tool("env_probe", json!({"key": "CARGO_MANIFEST_DIR"}))
        .await
        .expect("probe leak");
    assert_eq!(leaked.text, "unset", "host env leaked into MCP child");

    // Caller-supplied env survives the scrub.
    let explicit = client
        .call_tool("env_probe", json!({"key": "XG_EXPLICIT"}))
        .await
        .expect("probe explicit");
    assert_eq!(explicit.text, "set: visible");

    // Allowlisted PATH is passed through (or defaulted) — child runtimes
    // must still resolve binaries after the scrub.
    let path = client
        .call_tool("env_probe", json!({"key": "PATH"}))
        .await
        .expect("probe path");
    assert!(path.text.starts_with("set:"), "PATH missing in child");

    client.shutdown().await.expect("shutdown");
}
