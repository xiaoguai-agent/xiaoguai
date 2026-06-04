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
    let key = McpKey::new("fs", "1.0.0");

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
    let key = McpKey::new("fs", "1.0.0");
    sup.start(key.clone(), spawn_client().await).await.unwrap();
    sup.start(key.clone(), spawn_client().await).await.unwrap();
    assert_eq!(sup.list_active().len(), 1);
}

#[tokio::test]
async fn distinct_keys_are_independent() {
    let sup = McpSupervisor::new();
    sup.start(McpKey::new("fs", "1.0.0"), spawn_client().await)
        .await
        .unwrap();
    sup.start(McpKey::new("fs", "2.0.0"), spawn_client().await)
        .await
        .unwrap();
    assert_eq!(sup.list_active().len(), 2);
}

#[tokio::test]
async fn stop_unknown_key_is_noop() {
    let sup = McpSupervisor::new();
    let key = McpKey::new("missing", "9.9");
    sup.stop(&key).await.expect("noop");
}

/// v0.9.4.1: `reload_from_db` picks up newly inserted rows and stops rows
/// that disappeared since the last reload. Uses an in-memory repository
/// so the test stays Docker-free; spawn goes through the real
/// `mock-mcp-server` fixture (same path `StdioMcpClient` takes in
/// production).
mod reload_from_db {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use xiaoguai_storage::repositories::{McpServerRepository, RepoResult};
    use xiaoguai_types::{ids::McpServerInstanceId, McpServer, McpTransport};

    #[derive(Default)]
    struct InMemRepo {
        rows: Mutex<Vec<McpServer>>,
    }

    #[async_trait]
    impl McpServerRepository for InMemRepo {
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
            Ok(self.rows.lock().clone())
        }
        async fn delete(&self, id: &str) -> RepoResult<()> {
            self.rows.lock().retain(|s| s.id.as_str() != id);
            Ok(())
        }
    }

    fn fs_row(name: &str) -> McpServer {
        let now = chrono::Utc::now();
        McpServer {
            id: McpServerInstanceId::new(),
            name: name.into(),
            version: "1.0.0".into(),
            transport: McpTransport::Stdio,
            command: Some(fixture_bin().to_string()),
            args: vec![],
            env_keys: vec![],
            endpoint: None,
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn picks_up_new_row_and_stops_removed_one() {
        let repo = InMemRepo::default();
        repo.create(&fs_row("fs-one")).await.unwrap();

        let sup = McpSupervisor::new();
        let started = sup.reload_from_db(&repo).await.unwrap();
        assert_eq!(started.len(), 1, "first reload should start fs-one");
        assert_eq!(sup.list_active().len(), 1);

        // Add a second row, call reload — only fs-two should be newly
        // started, fs-one already live.
        repo.create(&fs_row("fs-two")).await.unwrap();
        let started = sup.reload_from_db(&repo).await.unwrap();
        assert_eq!(started.len(), 1);
        assert_eq!(sup.list_active().len(), 2);

        // Drop fs-one from the repo, reload again — supervisor should
        // stop it but keep fs-two.
        let ids: Vec<String> = repo
            .rows
            .lock()
            .iter()
            .filter(|s| s.name == "fs-one")
            .map(|s| s.id.as_str().to_string())
            .collect();
        for id in ids {
            repo.delete(&id).await.unwrap();
        }
        let started = sup.reload_from_db(&repo).await.unwrap();
        assert!(started.is_empty(), "no new rows on third reload");
        let live: Vec<_> = sup
            .list_active()
            .into_iter()
            .map(|k| k.server_name)
            .collect();
        assert_eq!(live, vec!["fs-two".to_string()]);
    }

    #[tokio::test]
    async fn disabled_rows_are_not_started() {
        let repo = InMemRepo::default();
        let mut row = fs_row("fs-disabled");
        row.enabled = false;
        repo.create(&row).await.unwrap();

        let sup = McpSupervisor::new();
        let started = sup.reload_from_db(&repo).await.unwrap();
        assert!(started.is_empty(), "disabled rows should be skipped");
        assert_eq!(sup.list_active().len(), 0);
    }
}
