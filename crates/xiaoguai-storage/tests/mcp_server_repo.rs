//! Integration tests for `PgMcpServerRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! Single-owner deployment: uniqueness is on `(name, version)`.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{McpServerRepository, PgMcpServerRepository, RepoError};
use xiaoguai_types::{ids::McpServerInstanceId, McpServer, McpTransport};

fn sample_stdio(name: &str, ver: &str) -> McpServer {
    let now = Utc::now().trunc_subsecs(6);
    McpServer {
        id: McpServerInstanceId::new(),
        name: name.into(),
        version: ver.into(),
        transport: McpTransport::Stdio,
        command: Some("npx".into()),
        args: vec![
            "-y".into(),
            "@modelcontextprotocol/server-filesystem".into(),
        ],
        env_keys: vec!["FS_ROOT".into()],
        endpoint: None,
        enabled: true,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn create_and_find_by_id() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    let server = sample_stdio("fs", "1.0.0");
    repo.create(&server).await.expect("create");
    let found = repo
        .find_by_id(server.id.as_str())
        .await
        .expect("query")
        .expect("present");
    assert_eq!(found.name, "fs");
    assert_eq!(found.transport, McpTransport::Stdio);
    assert_eq!(found.args.len(), 2);
    assert_eq!(found.env_keys, vec!["FS_ROOT"]);
    assert!(found.enabled);
}

#[tokio::test]
async fn list_returns_all_rows() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(&sample_stdio("a", "1.0"))
        .await
        .unwrap();
    repo.create(&sample_stdio("b", "1.0"))
        .await
        .unwrap();

    let rows = repo.list().await.unwrap();
    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[tokio::test]
async fn duplicate_name_version_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(&sample_stdio("fs", "1.0.0"))
        .await
        .unwrap();
    let err = repo
        .create(&sample_stdio("fs", "1.0.0"))
        .await
        .unwrap_err();
    assert!(matches!(err, RepoError::DuplicateKey(_)), "{err:?}");
}

#[tokio::test]
async fn same_name_different_version_ok() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(&sample_stdio("fs", "1.0.0"))
        .await
        .unwrap();
    repo.create(&sample_stdio("fs", "1.1.0"))
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    let s = sample_stdio("d", "1.0");
    repo.create(&s).await.unwrap();
    repo.delete(s.id.as_str()).await.unwrap();
    repo.delete(s.id.as_str()).await.unwrap();
    assert!(repo
        .find_by_id(s.id.as_str())
        .await
        .unwrap()
        .is_none());
}
