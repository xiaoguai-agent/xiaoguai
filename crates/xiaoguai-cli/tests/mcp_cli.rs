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
        let key_scope = server.tenant_id.as_ref().map(|t| t.as_str().to_string());
        let mut rows = self.rows.lock();
        if rows.values().any(|r| {
            r.name == server.name
                && r.version == server.version
                && r.tenant_id.as_ref().map(|t| t.as_str().to_string()) == key_scope
        }) {
            return Err(RepoError::DuplicateKey(format!(
                "({:?},{},{})",
                key_scope, server.name, server.version
            )));
        }
        rows.insert(server.id.as_str().to_string(), server.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>> {
        Ok(self.rows.lock().get(id).cloned())
    }

    async fn list_global(&self) -> RepoResult<Vec<McpServer>> {
        let mut out: Vec<_> = self
            .rows
            .lock()
            .values()
            .filter(|s| s.tenant_id.is_none())
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
        Ok(out)
    }

    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<McpServer>> {
        let mut out: Vec<_> = self
            .rows
            .lock()
            .values()
            .filter(|s| {
                s.tenant_id.is_none()
                    || s.tenant_id
                        .as_ref()
                        .is_some_and(|t| t.as_str() == tenant_id)
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.tenant_id
                .is_some()
                .cmp(&b.tenant_id.is_some())
                .then(a.name.cmp(&b.name))
                .then(a.version.cmp(&b.version))
        });
        Ok(out)
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        self.rows.lock().remove(id);
        Ok(())
    }
}

fn args_stdio(name: &str, tenant: Option<&str>) -> RegisterArgs {
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
        tenant: tenant.map(str::to_string),
    }
}

#[tokio::test]
async fn register_stdio_succeeds() {
    let repo = MemoryRepo::default();
    let s = register(&repo, args_stdio("fs", None)).await.expect("ok");
    assert_eq!(s.name, "fs");
    assert_eq!(s.transport, McpTransport::Stdio);
    assert!(s.id.as_str().starts_with("mcp_"));
    assert!(s.enabled);
}

#[tokio::test]
async fn register_rejects_unknown_transport() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x", None);
    args.transport = "websocket".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("unknown transport"));
}

#[tokio::test]
async fn register_stdio_requires_command() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x", None);
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
        tenant: None,
    };
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--endpoint"));
}

#[tokio::test]
async fn register_rejects_empty_name() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x", None);
    args.name = "  ".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--name"));
}

#[tokio::test]
async fn register_rejects_empty_version() {
    let repo = MemoryRepo::default();
    let mut args = args_stdio("x", None);
    args.version = String::new();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--version"));
}

#[tokio::test]
async fn duplicate_in_scope_rejected() {
    let repo = MemoryRepo::default();
    register(&repo, args_stdio("dup", None)).await.unwrap();
    let err = register(&repo, args_stdio("dup", None))
        .await
        .expect_err("dup");
    let s = err.to_string();
    assert!(
        s.contains("duplicate") || s.contains("DuplicateKey"),
        "got: {s}"
    );
}

#[tokio::test]
async fn list_for_tenant_includes_globals() {
    let repo = MemoryRepo::default();
    register(&repo, args_stdio("global", None)).await.unwrap();
    register(&repo, args_stdio("alpha-only", Some("ten_alpha")))
        .await
        .unwrap();
    register(&repo, args_stdio("beta-only", Some("ten_beta")))
        .await
        .unwrap();
    let rows = list(
        &repo,
        ListArgs {
            tenant: Some("ten_alpha".into()),
        },
    )
    .await
    .unwrap();
    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"global"));
    assert!(names.contains(&"alpha-only"));
    assert!(!names.contains(&"beta-only"));
}

#[tokio::test]
async fn remove_is_idempotent() {
    let repo = MemoryRepo::default();
    let s = register(&repo, args_stdio("rm", None)).await.unwrap();
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
    register(&repo, args_stdio("fs", None)).await.unwrap();
    let rows = list(&repo, ListArgs::default()).await.unwrap();
    let table = format_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("stdio"));
    assert!(table.contains("fs"));
    assert!(table.contains("global"));
}
