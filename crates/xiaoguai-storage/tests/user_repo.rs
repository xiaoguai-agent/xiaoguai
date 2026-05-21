//! Integration tests for `PgUserRepository`.
//!
//! These tests require a Docker daemon. Run with
//! `cargo test -p xiaoguai-storage --test user_repo -- --ignored`.

mod common;

use chrono::{SubsecRound, Utc};
use xiaoguai_storage::repositories::{
    PgTenantRepository, PgUserRepository, RepoError, TenantRepository, UserRepository,
};
use xiaoguai_types::{
    ids::{TenantId, UserId},
    Tenant, TenantRole as Role, TenantStatus, User,
};

async fn seed_tenant(pool: &sqlx::PgPool, name: &str) -> Tenant {
    let repo = PgTenantRepository::new(pool.clone());
    let tenant = Tenant {
        id: TenantId::new(),
        name: name.to_string(),
        display_name: name.to_string(),
        created_at: Utc::now().trunc_subsecs(6),
        status: TenantStatus::Active,
    };
    repo.create(&tenant).await.expect("seed tenant");
    tenant
}

fn sample_user(tenant_id: &TenantId, email: &str, roles: Vec<Role>) -> User {
    User {
        id: UserId::new(),
        tenant_id: tenant_id.clone(),
        email: email.to_string(),
        display_name: email.to_string(),
        roles,
        created_at: Utc::now().trunc_subsecs(6),
        last_login_at: None,
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_and_find_by_id_round_trip() {
    let (pool, _pg) = common::test_setup().await;
    let tenant = seed_tenant(&pool, "t-create").await;
    let repo = PgUserRepository::new(pool);

    let user = sample_user(
        &tenant.id,
        "alice@example.com",
        vec![Role::TenantAdmin, Role::Member],
    );
    repo.create(&user).await.expect("create");

    let found = repo
        .find_by_id(user.id.as_str())
        .await
        .expect("find")
        .expect("present");

    assert_eq!(found.id.as_str(), user.id.as_str());
    assert_eq!(found.tenant_id.as_str(), tenant.id.as_str());
    assert_eq!(found.email, "alice@example.com");
    assert_eq!(found.roles.len(), 2);
    assert!(found.roles.contains(&Role::TenantAdmin));
    assert!(found.roles.contains(&Role::Member));
    assert!(found.last_login_at.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn find_by_id_returns_none_when_missing() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgUserRepository::new(pool);

    let result = repo.find_by_id("usr_missing").await.expect("query");
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn find_by_email_scoped_to_tenant() {
    let (pool, _pg) = common::test_setup().await;
    let tenant_a = seed_tenant(&pool, "tenant-a").await;
    let tenant_b = seed_tenant(&pool, "tenant-b").await;
    let repo = PgUserRepository::new(pool);

    let user_a = sample_user(&tenant_a.id, "shared@example.com", vec![Role::Member]);
    let user_b = sample_user(&tenant_b.id, "shared@example.com", vec![Role::Member]);
    repo.create(&user_a).await.expect("create a");
    repo.create(&user_b).await.expect("create b");

    let from_a = repo
        .find_by_email(tenant_a.id.as_str(), "shared@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(from_a.id.as_str(), user_a.id.as_str());

    let from_b = repo
        .find_by_email(tenant_b.id.as_str(), "shared@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(from_b.id.as_str(), user_b.id.as_str());

    let missing = repo
        .find_by_email(tenant_a.id.as_str(), "nobody@example.com")
        .await
        .expect("query");
    assert!(missing.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_by_tenant_pagination() {
    let (pool, _pg) = common::test_setup().await;
    let tenant = seed_tenant(&pool, "page-tenant").await;
    let repo = PgUserRepository::new(pool);

    for i in 0..4 {
        let mut u = sample_user(&tenant.id, &format!("u{i}@x.com"), vec![Role::Member]);
        u.created_at = Utc::now().trunc_subsecs(6) + chrono::Duration::milliseconds(i);
        repo.create(&u).await.expect("create");
    }

    let page1 = repo
        .list_by_tenant(tenant.id.as_str(), 2, 0)
        .await
        .expect("list");
    let page2 = repo
        .list_by_tenant(tenant.id.as_str(), 2, 2)
        .await
        .expect("list");

    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    assert_ne!(page1[0].id.as_str(), page2[0].id.as_str());
    for u in page1.iter().chain(page2.iter()) {
        assert_eq!(u.tenant_id.as_str(), tenant.id.as_str());
        assert_eq!(u.roles, vec![Role::Member]);
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_then_find_returns_none_and_is_idempotent() {
    let (pool, _pg) = common::test_setup().await;
    let tenant = seed_tenant(&pool, "delete-tenant").await;
    let repo = PgUserRepository::new(pool);

    let user = sample_user(&tenant.id, "bye@example.com", vec![Role::Member]);
    repo.create(&user).await.expect("create");

    repo.delete(user.id.as_str()).await.expect("delete");
    assert!(repo
        .find_by_id(user.id.as_str())
        .await
        .expect("query")
        .is_none());

    // Second delete is a no-op.
    repo.delete(user.id.as_str()).await.expect("idempotent");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn duplicate_email_in_same_tenant_is_rejected() {
    let (pool, _pg) = common::test_setup().await;
    let tenant = seed_tenant(&pool, "dup-tenant").await;
    let repo = PgUserRepository::new(pool);

    let u1 = sample_user(&tenant.id, "dup@example.com", vec![Role::Member]);
    let u2 = sample_user(&tenant.id, "dup@example.com", vec![Role::Member]);
    repo.create(&u1).await.expect("first");

    let err = repo.create(&u2).await.expect_err("should fail");
    assert!(
        matches!(err, RepoError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn record_login_updates_last_login_at() {
    let (pool, _pg) = common::test_setup().await;
    let tenant = seed_tenant(&pool, "login-tenant").await;
    let repo = PgUserRepository::new(pool);

    let user = sample_user(&tenant.id, "login@example.com", vec![Role::Member]);
    repo.create(&user).await.expect("create");

    repo.record_login(user.id.as_str()).await.expect("login");
    let found = repo
        .find_by_id(user.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert!(found.last_login_at.is_some());

    // Unknown user → NotFound.
    let err = repo
        .record_login("usr_missing")
        .await
        .expect_err("should fail");
    assert!(matches!(err, RepoError::NotFound), "got {err:?}");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_with_unknown_tenant_returns_foreign_key_error() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgUserRepository::new(pool);
    let fake_tenant = TenantId::new();
    let user = sample_user(&fake_tenant, "orphan@example.com", vec![Role::Member]);

    let err = repo.create(&user).await.expect_err("should fail");
    assert!(
        matches!(err, RepoError::ForeignKey(_)),
        "expected ForeignKey, got {err:?}"
    );
}
