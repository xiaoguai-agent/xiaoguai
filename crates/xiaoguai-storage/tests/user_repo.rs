//! Integration tests for `SqliteUserRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{RepoError, SqliteUserRepository, UserRepository};
use xiaoguai_types::{ids::UserId, TenantRole as Role, User};

fn sample_user(email: &str, roles: Vec<Role>) -> User {
    User {
        id: UserId::new(),
        email: email.to_string(),
        display_name: email.to_string(),
        roles,
        created_at: Utc::now().trunc_subsecs(6),
        last_login_at: None,
    }
}

#[tokio::test]
async fn create_and_find_by_id_round_trip() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let user = sample_user("alice@example.com", vec![Role::TenantAdmin, Role::Member]);
    repo.create(&user).await.expect("create");

    let found = repo
        .find_by_id(user.id.as_str())
        .await
        .expect("find")
        .expect("present");

    assert_eq!(found.id.as_str(), user.id.as_str());
    assert_eq!(found.email, "alice@example.com");
    assert_eq!(found.roles.len(), 2);
    assert!(found.roles.contains(&Role::TenantAdmin));
    assert!(found.roles.contains(&Role::Member));
    assert!(found.last_login_at.is_none());
}

#[tokio::test]
async fn find_by_id_returns_none_when_missing() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let result = repo.find_by_id("usr_missing").await.expect("query");
    assert!(result.is_none());
}

#[tokio::test]
async fn find_by_email_round_trip() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let user = sample_user("shared@example.com", vec![Role::Member]);
    repo.create(&user).await.expect("create");

    let found = repo
        .find_by_email("shared@example.com")
        .await
        .expect("find")
        .expect("present");
    assert_eq!(found.id.as_str(), user.id.as_str());

    let missing = repo
        .find_by_email("nobody@example.com")
        .await
        .expect("query");
    assert!(missing.is_none());
}

#[tokio::test]
async fn list_pagination() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    for i in 0..4 {
        let mut u = sample_user(&format!("u{i}@x.com"), vec![Role::Member]);
        u.created_at = Utc::now().trunc_subsecs(6) + chrono::Duration::milliseconds(i);
        repo.create(&u).await.expect("create");
    }

    let page1 = repo.list(2, 0).await.expect("list");
    let page2 = repo.list(2, 2).await.expect("list");

    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    assert_ne!(page1[0].id.as_str(), page2[0].id.as_str());
    for u in page1.iter().chain(page2.iter()) {
        assert_eq!(u.roles, vec![Role::Member]);
    }
}

#[tokio::test]
async fn delete_then_find_returns_none_and_is_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let user = sample_user("bye@example.com", vec![Role::Member]);
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
async fn duplicate_email_is_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let u1 = sample_user("dup@example.com", vec![Role::Member]);
    let u2 = sample_user("dup@example.com", vec![Role::Member]);
    repo.create(&u1).await.expect("first");

    let err = repo.create(&u2).await.expect_err("should fail");
    assert!(
        matches!(err, RepoError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );
}

#[tokio::test]
async fn record_login_updates_last_login_at() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteUserRepository::new(pool);

    let user = sample_user("login@example.com", vec![Role::Member]);
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
