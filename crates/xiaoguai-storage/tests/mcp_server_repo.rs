//! Integration tests for `PgMcpServerRepository` against testcontainers PG.
//!
//! `#[ignore = "requires Docker"]` like all other repo tests in this crate.

mod common;

use chrono::{SubsecRound, Utc};
use xiaoguai_storage::repositories::{
    McpServerRepository, PgMcpServerRepository, PgTenantRepository, RepoError, TenantRepository,
};
use xiaoguai_types::{
    ids::{McpServerInstanceId, TenantId},
    McpServer, McpTransport, Tenant, TenantStatus,
};

fn sample_tenant(name: &str) -> Tenant {
    Tenant {
        id: TenantId::new(),
        name: name.into(),
        display_name: format!("Display {name}"),
        created_at: Utc::now().trunc_subsecs(6),
        status: TenantStatus::Active,
    }
}

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
#[ignore = "requires Docker"]
async fn create_and_find_by_id_global() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    let server = sample_stdio("fs", "1.0.0", None);
    repo.create(None, &server).await.expect("create");
    let found = repo
        .find_by_id(None, server.id.as_str())
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
#[ignore = "requires Docker"]
async fn list_for_tenant_filters_correctly() {
    let (pool, _pg) = common::test_setup().await;
    let trepo = PgTenantRepository::new(pool.clone());
    let t1 = sample_tenant("t1");
    let t2 = sample_tenant("t2");
    trepo.create(&t1).await.unwrap();
    trepo.create(&t2).await.unwrap();

    let repo = PgMcpServerRepository::new(pool);
    repo.create(None, &sample_stdio("global", "1.0", None))
        .await
        .unwrap();
    repo.create(None, &sample_stdio("t1-only", "1.0", Some(t1.id.clone())))
        .await
        .unwrap();
    repo.create(None, &sample_stdio("t2-only", "1.0", Some(t2.id.clone())))
        .await
        .unwrap();

    let rows = repo.list_for_tenant(t1.id.as_str()).await.unwrap();
    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"global"));
    assert!(names.contains(&"t1-only"));
    assert!(!names.contains(&"t2-only"), "leaked t2: {names:?}");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn duplicate_name_version_in_scope_rejected() {
    let (pool, _pg) = common::test_setup().await;
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
#[ignore = "requires Docker"]
async fn same_name_different_version_ok() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgMcpServerRepository::new(pool);
    repo.create(None, &sample_stdio("fs", "1.0.0", None))
        .await
        .unwrap();
    repo.create(None, &sample_stdio("fs", "1.1.0", None))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_idempotent() {
    let (pool, _pg) = common::test_setup().await;
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
