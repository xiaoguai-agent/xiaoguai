//! Unit tests for the `mcp` subcommand business logic — in-memory repo,
//! no Docker, no real PG.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use xiaoguai_cli::commands::mcp::{
    format_table, list, register, remove, ListArgs, RegisterArgs, RemoveArgs,
};
use xiaoguai_storage::repositories::{McpServerRepository, RepoError, RepoResult};
use xiaoguai_types::{McpServer, McpTransport};

#[derive(Default, Clone)]
struct MemoryRepo {
    rows: Arc<Mutex<HashMap<String, McpServer>>>,
}

#[async_trait]
impl McpServerRepository for MemoryRepo {
    async fn create(&self, server: &McpServer) -> RepoResult<()> {
        let mut rows = self.rows.lock();
        if rows
            .values()
            .any(|r| r.name == server.name && r.version == server.version)
        {
            return Err(RepoError::DuplicateKey(format!(
                "({},{})",
                server.name, server.version
            )));
        }
        rows.insert(server.id.as_str().to_string(), server.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>> {
        Ok(self.rows.lock().get(id).cloned())
    }

    async fn list(&self) -> RepoResult<Vec<McpServer>> {
        let mut out: Vec<_> = self.rows.lock().values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
        Ok(out)
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        self.rows.lock().remove(id);
        Ok(())
    }
}

fn args_stdio(name: &str) -> RegisterArgs {
    RegisterArgs {
        name: name.into(),
        version: "1.0.0".into(),
        transport: "stdio".into(),
        command: Some("npx".into()),
        args: vec![
            "-y".into(),
            "@modelcontextprotocol/server-filesystem".into(),
        ],
        env_keys: vec!["FS_ROOT".into()],
        endpoint: None,
    }
}

#[tokio::test]
async fn register_stdio_succeeds() {
    let repo = MemoryRepo::default();
    let s = register(&repo, args_stdio("fs")).await.expect("ok");
    assert_eq!(s.name, "fs");
    assert_eq!(s.transport, McpTransport::Stdio);
    assert!(s.id.as_str().starts_with("mcp_"));
    assert!(s.enabled);
}

#[tokio::test]
async fn register_rejects_unknown_transport() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x");
    args.transport = "websocket".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("unknown transport"));
}

#[tokio::test]
async fn register_stdio_requires_command() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x");
    args.command = None;
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--command"));
}

#[tokio::test]
async fn register_http_requires_endpoint() {
    let repo = MemoryRepo::default();
    let args = RegisterArgs {
        name: "remote".into(),
        version: "1.0".into(),
        transport: "http".into(),
        command: None,
        args: vec![],
        env_keys: vec![],
        endpoint: None,
    };
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--endpoint"));
}

#[tokio::test]
async fn register_rejects_empty_name() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x");
    args.name = "  ".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--name"));
}

#[tokio::test]
async fn register_rejects_empty_version() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x");
    args.version = String::new();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--version"));
}

#[tokio::test]
async fn duplicate_in_scope_rejected() {
    let repo = MemoryRepo::default();
    register(&repo, args_stdio("dup")).await.unwrap();
    let err = register(&repo, args_stdio("dup"))
        .await
        .expect_err("dup");
    let s = err.to_string();
    assert!(
        s.contains("duplicate") || s.contains("DuplicateKey"),
        "got: {s}"
    );
}

#[tokio::test]
async fn remove_is_idempotent() {
    let repo = MemoryRepo::default();
    let s = register(&repo, args_stdio("rm")).await.unwrap();
    remove(
        &repo,
        RemoveArgs {
            id: s.id.as_str().to_string(),
        },
    )
    .await
    .unwrap();
    remove(
        &repo,
        RemoveArgs {
            id: s.id.as_str().to_string(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn remove_rejects_empty_id() {
    let repo = MemoryRepo::default();
    let err = remove(
        &repo,
        RemoveArgs {
            id: "   ".to_string(),
        },
    )
    .await
    .expect_err("should fail");
    assert!(err.to_string().contains("--id"));
}

#[tokio::test]
async fn format_table_renders_rows() {
    let repo = MemoryRepo::default();
    register(&repo, args_stdio("fs")).await.unwrap();
    let rows = list(&repo, ListArgs::default()).await.unwrap();
    let table = format_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("stdio"));
    assert!(table.contains("fs"));
}
