//! End-to-end: spawn `npx -y @modelcontextprotocol/server-filesystem`
//! and verify the reference impl's tool surface is reachable.
//!
//! `#[ignore]` — requires Node.js ≥ 18 and a network round-trip on the
//! first run (npx warms its cache). Run with:
//!
//!     cargo test -p xiaoguai-mcp --test fs_server_e2e -- --ignored

use std::path::PathBuf;
use xiaoguai_mcp::{McpClient, StdioMcpClient};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/xiaoguai-mcp/; jump up two levels.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[tokio::test]
#[ignore = "requires Node.js + network for first npx run"]
async fn filesystem_server_lists_expected_tools() {
    // Scope the FS server to the workspace root so the test never touches
    // the developer's home directory.
    let root = workspace_root();
    let root_str = root.to_string_lossy().to_string();

    let envs: [(&str, &str); 0] = [];
    let client = StdioMcpClient::spawn(
        "npx",
        &["-y", "@modelcontextprotocol/server-filesystem", &root_str],
        &envs,
    )
    .await
    .expect("spawn npx");

    let info = client.initialize().await.expect("init");
    // Reference server identifies itself; pin only the substring.
    assert!(
        info.name.to_lowercase().contains("filesystem")
            || info.name.to_lowercase().contains("file-system"),
        "unexpected server name: {}",
        info.name
    );

    let tools = client.list_tools().await.expect("list_tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    // Stable subset of the reference impl's tool set. A minor version bump
    // can add tools (fine); removing any of these is intentional breakage.
    for required in ["read_file", "list_directory", "search_files"] {
        assert!(
            names.contains(&required),
            "missing reference tool {required}: have {names:?}"
        );
    }

    client.shutdown().await.ok();
}
