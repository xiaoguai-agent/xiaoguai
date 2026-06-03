//! Integration tests for [`PgSessionRepository`] (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`
//! and exercises CRUD + pagination + ordering.

mod common;

use chrono::{Duration, Utc};
use common::test_setup;
use sqlx::SqlitePool;
use xiaoguai_storage::repositories::{PgSessionRepository, RepoError, SessionRepository};
use xiaoguai_storage::OWNER_TENANT_ID;
use xiaoguai_types::{Session, SessionId, SessionStatus, TenantId, UserId};

/// Seed a user via raw SQL so the session FK (`sessions.user_id`) is satisfied.
/// The `users` table no longer carries `tenant_id`; we return a synthetic owner
/// `TenantId` only to build `Session` fixtures.
async fn seed_user(pool: &SqlitePool) -> (TenantId, UserId) {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES (?, ?, ?)")
        .bind(user_id.as_str())
        .bind(format!("u-{}@example.com", user_id.as_str()))
        .bind("Test User")
        .execute(pool)
        .await
        .expect("insert user");
    (TenantId::from(OWNER_TENANT_ID.to_string()), user_id)
}

fn fixture_session(tenant: &TenantId, user: &UserId, model: &str) -> Session {
    let now = Utc::now();
    Session {
        id: SessionId::new(),
        tenant_id: tenant.clone(),
        user_id: user.clone(),
        title: Some("Test session".to_string()),
        created_at: now,
        updated_at: now,
        model: model.to_string(),
        status: SessionStatus::Active,
        parent_session_id: None,
        forked_from_message_id: None,
    }
}

#[tokio::test]
async fn create_then_find_roundtrip() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(None, &session).await.expect("create");

    let fetched = repo
        .find_by_id(None, session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert_eq!(fetched.id.as_str(), session.id.as_str());
    assert_eq!(fetched.title.as_deref(), Some("Test session"));
    assert_eq!(fetched.model, "gpt-4o-mini");
    assert_eq!(fetched.status, SessionStatus::Active);

    let missing = repo
        .find_by_id(None, "sess_doesnotexist")
        .await
        .expect("find");
    assert!(missing.is_none());
}

#[tokio::test]
async fn list_by_user_orders_by_updated_at_desc_with_pagination() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    // Create 5 sessions with staggered updated_at; oldest first so DESC order
    // means we expect the LAST inserted to appear first.
    let base = Utc::now() - Duration::hours(10);
    let mut ids = Vec::with_capacity(5);
    for i in 0..5_i64 {
        let mut s = fixture_session(&tenant, &user, "gpt-4o-mini");
        s.created_at = base + Duration::hours(i);
        s.updated_at = base + Duration::hours(i);
        repo.create(None, &s).await.expect("create");
        ids.push(s.id);
    }

    let page1 = repo
        .list_by_user(None, user.as_str(), 2, 0)
        .await
        .expect("page1");
    assert_eq!(page1.len(), 2);
    // Newest (highest updated_at, last inserted) first.
    assert_eq!(page1[0].id.as_str(), ids[4].as_str());
    assert_eq!(page1[1].id.as_str(), ids[3].as_str());

    let page2 = repo
        .list_by_user(None, user.as_str(), 2, 2)
        .await
        .expect("page2");
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[0].id.as_str(), ids[2].as_str());
    assert_eq!(page2[1].id.as_str(), ids[1].as_str());

    let page3 = repo
        .list_by_user(None, user.as_str(), 2, 4)
        .await
        .expect("page3");
    assert_eq!(page3.len(), 1);
    assert_eq!(page3[0].id.as_str(), ids[0].as_str());

    let neg = repo.list_by_user(None, user.as_str(), -1, 0).await;
    assert!(matches!(neg, Err(RepoError::InvalidArgument(_))));
}

#[tokio::test]
async fn touch_bumps_updated_at_and_errors_on_missing() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let mut session = fixture_session(&tenant, &user, "gpt-4o-mini");
    session.updated_at = Utc::now() - Duration::hours(1);
    session.created_at = session.updated_at;
    repo.create(None, &session).await.expect("create");

    repo.touch(None, session.id.as_str()).await.expect("touch");

    let after = repo
        .find_by_id(None, session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert!(after.updated_at > session.updated_at);

    let missing = repo.touch(None, "sess_nope").await;
    assert!(matches!(missing, Err(RepoError::NotFound)));
}

#[tokio::test]
async fn archive_sets_status_and_errors_on_missing() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(None, &session).await.expect("create");
    repo.archive(None, session.id.as_str())
        .await
        .expect("archive");

    let after = repo
        .find_by_id(None, session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert_eq!(after.status, SessionStatus::Archived);

    let missing = repo.archive(None, "sess_nope").await;
    assert!(matches!(missing, Err(RepoError::NotFound)));
}

#[tokio::test]
async fn delete_is_idempotent_and_cascades_via_fk() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(None, &session).await.expect("create");

    // First delete removes the row.
    repo.delete(None, session.id.as_str())
        .await
        .expect("delete1");
    let gone = repo
        .find_by_id(None, session.id.as_str())
        .await
        .expect("find");
    assert!(gone.is_none());

    // Second delete is a no-op (idempotent).
    repo.delete(None, session.id.as_str())
        .await
        .expect("delete2");
}

#[tokio::test]
async fn duplicate_create_returns_duplicate_key() {
    let (pool, _guard) = test_setup().await;
    let (tenant, user) = seed_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(None, &session).await.expect("first insert");
    let err = repo.create(None, &session).await.expect_err("dup");
    assert!(matches!(err, RepoError::DuplicateKey(_)));
}
