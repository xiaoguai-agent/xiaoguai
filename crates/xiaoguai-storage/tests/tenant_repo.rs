//! Integration tests for `PgTenantRepository`.
//!
//! These tests require a Docker daemon. Run with
//! `cargo test -p xiaoguai-storage --test tenant_repo -- --ignored`.

mod common;

use chrono::{SubsecRound, Utc};
use xiaoguai_storage::repositories::{PgTenantRepository, RepoError, TenantRepository};
use xiaoguai_types::{ids::TenantId, Tenant, TenantStatus};

fn sample_tenant(name: &str) -> Tenant {
    Tenant {
        id: TenantId::new(),
        name: name.to_string(),
        display_name: format!("Display {name}"),
        // Round-trip to microsecond precision so Postgres `TIMESTAMPTZ`
        // comparisons are exact.
        created_at: Utc::now().trunc_subsecs(6),
        status: TenantStatus::Active,
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_and_find_by_id_round_trip() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);
    let tenant = sample_tenant("acme");

    repo.create(&tenant).await.expect("create");
    let found = repo
        .find_by_id(tenant.id.as_str())
        .await
        .expect("find")
        .expect("present");

    assert_eq!(found.id.as_str(), tenant.id.as_str());
    assert_eq!(found.name, "acme");
    assert_eq!(found.display_name, "Display acme");
    assert_eq!(found.status, TenantStatus::Active);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn find_by_id_returns_none_when_missing() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);

    let result = repo.find_by_id("ten_does_not_exist").await.expect("query");
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn find_by_name_locates_existing_tenant() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);
    let tenant = sample_tenant("globex");
    repo.create(&tenant).await.expect("create");

    let found = repo
        .find_by_name("globex")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(found.id.as_str(), tenant.id.as_str());

    let missing = repo.find_by_name("nope").await.expect("query");
    assert!(missing.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_respects_limit_and_offset() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);

    for i in 0..5 {
        // small sleep ensures distinct created_at ordering
        let mut t = sample_tenant(&format!("ten-{i}"));
        // Force a deterministic order by stamping created_at relative to i.
        t.created_at = Utc::now().trunc_subsecs(6) + chrono::Duration::milliseconds(i);
        repo.create(&t).await.expect("create");
    }

    let page1 = repo.list(2, 0).await.expect("list");
    let page2 = repo.list(2, 2).await.expect("list");
    let page3 = repo.list(2, 4).await.expect("list");

    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    assert_eq!(page3.len(), 1);

    // Pages should not overlap.
    let id1 = page1[0].id.as_str().to_string();
    let id2 = page2[0].id.as_str().to_string();
    assert_ne!(id1, id2);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_then_find_returns_none_and_is_idempotent() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);
    let tenant = sample_tenant("initech");
    repo.create(&tenant).await.expect("create");

    repo.delete(tenant.id.as_str()).await.expect("delete");
    assert!(repo
        .find_by_id(tenant.id.as_str())
        .await
        .expect("query")
        .is_none());

    // Second delete is a no-op.
    repo.delete(tenant.id.as_str()).await.expect("idempotent");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_duplicate_id_returns_duplicate_key() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);
    let tenant = sample_tenant("dup");
    repo.create(&tenant).await.expect("first create");

    // Same id, different name still collides on the primary key.
    let mut clash = tenant.clone();
    clash.name = "dup2".to_string();

    let err = repo.create(&clash).await.expect_err("should fail");
    assert!(
        matches!(err, RepoError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_duplicate_name_returns_duplicate_key() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTenantRepository::new(pool);
    let mut t1 = sample_tenant("shared-name");
    let mut t2 = sample_tenant("shared-name");
    // Distinct IDs, identical name → violates the UNIQUE constraint on name.
    t1.id = TenantId::new();
    t2.id = TenantId::new();
    repo.create(&t1).await.expect("first");

    let err = repo.create(&t2).await.expect_err("should fail");
    assert!(matches!(err, RepoError::DuplicateKey(_)));
}
