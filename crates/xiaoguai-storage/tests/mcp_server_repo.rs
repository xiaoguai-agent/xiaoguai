//! Integration tests for `PgMcpServerRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! `tenant_id` is dropped on write and reads back as `None`; uniqueness is on
//! `(name, version)` across the single owner namespace.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{McpServerRepository, PgMcpServerRepository, RepoError};
use xiaoguai_storage::OWNER_TENANT_ID;
use xiaoguai_types::{
    ids::{McpServerInstanceId, TenantId},
    McpServer, McpTransport,
};

fn sample_stdio(name: &str, ver: &str, tenant: Option<TenantId>) -> McpServer {
    let now = Utc::now().trunc_subsecs(6);
    McpServer {
        id: McpServerInstanceId::new(),
        tenant_id: tenant,
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
    let server = sample_stdio("fs", "1.0.0", None);
    repo.create(None, &server).await.expect("create");
    let found = repo
        .find_by_id(None, server.id.as_str())
        .await
        .expect("query")
        .expect("present");
    assert_eq!(found.name, "fs");
    assert!(found.tenant_id.is_none());
    assert_eq!(found.transport, McpTransport::Stdio);
    assert_eq!(found.args.len(), 2);
    assert_eq!(found.env_keys, vec!["FS_ROOT"]);
    assert!(found.enabled);
}

#[tokio::test]
async fn list_returns_all_rows() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(None, &sample_stdio("a", "1.0", None))
        .await
        .unwrap();
    repo.create(None, &sample_stdio("b", "1.0", None))
        .await
        .unwrap();

    let rows = repo.list_for_tenant(OWNER_TENANT_ID).await.unwrap();
    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[tokio::test]
async fn duplicate_name_version_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(None, &sample_stdio("fs", "1.0.0", None))
        .await
        .unwrap();
    let err = repo
        .create(None, &sample_stdio("fs", "1.0.0", None))
        .await
        .unwrap_err();
    assert!(matches!(err, RepoError::DuplicateKey(_)), "{err:?}");
}

#[tokio::test]
async fn same_name_different_version_ok() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(None, &sample_stdio("fs", "1.0.0", None))
        .await
        .unwrap();
    repo.create(None, &sample_stdio("fs", "1.1.0", None))
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    let s = sample_stdio("d", "1.0", None);
    repo.create(None, &s).await.unwrap();
    repo.delete(None, s.id.as_str()).await.unwrap();
    repo.delete(None, s.id.as_str()).await.unwrap();
    assert!(repo
        .find_by_id(None, s.id.as_str())
        .await
        .unwrap()
        .is_none());
}
