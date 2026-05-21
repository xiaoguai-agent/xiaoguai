//! `McpSupervisor` minimal lifecycle: `start` / `get` / `stop` / `list_active`.
//!
//! Uses the in-process `mock-mcp-server` fixture (no Docker, no npx).

use std::sync::Arc;
use xiaoguai_mcp::{McpClient, McpKey, McpSupervisor, StdioMcpClient};

fn fixture_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mock-mcp-server")
}

async fn spawn_client() -> Arc<dyn McpClient> {
    let envs: [(&str, &str); 0] = [];
    Arc::new(
        StdioMcpClient::spawn(fixture_bin(), &[], &envs)
            .await
            .expect("spawn"),
    )
}

#[tokio::test]
async fn start_get_stop_round_trip() {
    let sup = McpSupervisor::new();
    let key = McpKey::new("ten_a", "fs", "1.0.0");

    sup.start(key.clone(), spawn_client().await).await.unwrap();
    let again = sup.get(&key).expect("registered");
    assert_eq!(again.list_tools().await.unwrap().len(), 1);
    assert_eq!(sup.list_active().len(), 1);

    sup.stop(&key).await.unwrap();
    assert!(sup.get(&key).is_none());
    assert_eq!(sup.list_active().len(), 0);
}

#[tokio::test]
async fn start_twice_same_key_replaces() {
    let sup = McpSupervisor::new();
    let key = McpKey::new("ten_a", "fs", "1.0.0");
    sup.start(key.clone(), spawn_client().await).await.unwrap();
    sup.start(key.clone(), spawn_client().await).await.unwrap();
    assert_eq!(sup.list_active().len(), 1);
}

#[tokio::test]
async fn distinct_keys_are_independent() {
    let sup = McpSupervisor::new();
    sup.start(McpKey::new("ten_a", "fs", "1.0.0"), spawn_client().await)
        .await
        .unwrap();
    sup.start(McpKey::new("ten_b", "fs", "1.0.0"), spawn_client().await)
        .await
        .unwrap();
    assert_eq!(sup.list_active().len(), 2);
}

#[tokio::test]
async fn stop_unknown_key_is_noop() {
    let sup = McpSupervisor::new();
    let key = McpKey::new("ten_z", "missing", "9.9");
    sup.stop(&key).await.expect("noop");
}
