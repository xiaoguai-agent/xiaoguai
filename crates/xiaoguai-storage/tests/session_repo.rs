//! Integration tests for [`PgSessionRepository`].
//!
//! These tests spin up a real Postgres in a container (testcontainers) and
//! exercise CRUD + pagination + ordering. They are marked `#[ignore]` so they
//! only run when Docker is available (`cargo test -- --ignored`).
//!
//! ## RLS caveat
//!
//! The container's `postgres` user is a SUPERUSER, which bypasses RLS unless
//! the policy is declared `FORCE`. Our migration does NOT use `FORCE`, so RLS
//! is effectively *advisory* in this test harness. Production code MUST run
//! as a non-superuser role; that path is covered by separate end-to-end tests
//! once a dedicated app role is provisioned.

#![cfg(test)]

use chrono::{Duration, Utc};
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use xiaoguai_storage::{
    db,
    repositories::{PgSessionRepository, RepoError, SessionRepository},
};
use xiaoguai_types::{Session, SessionId, SessionStatus, TenantId, UserId};

/// Local fallback for the shared `tests/common/mod.rs` helper owned by
/// sub-agent A. Function name kept identical (`test_setup`) so the file can
/// be merged with `mod common;` later without touching tests.
async fn test_setup() -> (PgPool, ContainerAsync<Postgres>) {
    let pg = Postgres::default().start().await.expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, pg)
}

/// Seed a tenant + user via raw SQL so we satisfy FK constraints without
/// reaching into other sub-agents' repos.
async fn seed_tenant_user(pool: &PgPool) -> (TenantId, UserId) {
    let tenant_id = TenantId::new();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO tenants (id, name, display_name) VALUES ($1, $2, $3)")
        .bind(tenant_id.as_str())
        .bind(format!("tenant-{}", tenant_id.as_str()))
        .bind("Test Tenant")
        .execute(pool)
        .await
        .expect("insert tenant");
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id.as_str())
    .bind(tenant_id.as_str())
    .bind(format!("u-{}@example.com", user_id.as_str()))
    .bind("Test User")
    .execute(pool)
    .await
    .expect("insert user");
    (tenant_id, user_id)
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
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_then_find_roundtrip() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(&session).await.expect("create");

    let fetched = repo
        .find_by_id(session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert_eq!(fetched.id.as_str(), session.id.as_str());
    assert_eq!(fetched.title.as_deref(), Some("Test session"));
    assert_eq!(fetched.model, "gpt-4o-mini");
    assert_eq!(fetched.status, SessionStatus::Active);

    let missing = repo.find_by_id("sess_doesnotexist").await.expect("find");
    assert!(missing.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_by_user_orders_by_updated_at_desc_with_pagination() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    // Create 5 sessions with staggered updated_at; oldest first so DESC order
    // means we expect the LAST inserted to appear first.
    let base = Utc::now() - Duration::hours(10);
    let mut ids = Vec::with_capacity(5);
    for i in 0..5_i64 {
        let mut s = fixture_session(&tenant, &user, "gpt-4o-mini");
        s.created_at = base + Duration::hours(i);
        s.updated_at = base + Duration::hours(i);
        repo.create(&s).await.expect("create");
        ids.push(s.id);
    }

    let page1 = repo.list_by_user(user.as_str(), 2, 0).await.expect("page1");
    assert_eq!(page1.len(), 2);
    // Newest (highest updated_at, last inserted) first.
    assert_eq!(page1[0].id.as_str(), ids[4].as_str());
    assert_eq!(page1[1].id.as_str(), ids[3].as_str());

    let page2 = repo.list_by_user(user.as_str(), 2, 2).await.expect("page2");
    assert_eq!(page2.len(), 2);
    assert_eq!(page2[0].id.as_str(), ids[2].as_str());
    assert_eq!(page2[1].id.as_str(), ids[1].as_str());

    let page3 = repo.list_by_user(user.as_str(), 2, 4).await.expect("page3");
    assert_eq!(page3.len(), 1);
    assert_eq!(page3[0].id.as_str(), ids[0].as_str());

    let neg = repo.list_by_user(user.as_str(), -1, 0).await;
    assert!(matches!(neg, Err(RepoError::InvalidArgument(_))));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn touch_bumps_updated_at_and_errors_on_missing() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let mut session = fixture_session(&tenant, &user, "gpt-4o-mini");
    session.updated_at = Utc::now() - Duration::hours(1);
    session.created_at = session.updated_at;
    repo.create(&session).await.expect("create");

    repo.touch(session.id.as_str()).await.expect("touch");

    let after = repo
        .find_by_id(session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert!(after.updated_at > session.updated_at);

    let missing = repo.touch("sess_nope").await;
    assert!(matches!(missing, Err(RepoError::NotFound)));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn archive_sets_status_and_errors_on_missing() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(&session).await.expect("create");
    repo.archive(session.id.as_str()).await.expect("archive");

    let after = repo
        .find_by_id(session.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert_eq!(after.status, SessionStatus::Archived);

    let missing = repo.archive("sess_nope").await;
    assert!(matches!(missing, Err(RepoError::NotFound)));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_is_idempotent_and_cascades_via_fk() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(&session).await.expect("create");

    // First delete removes the row.
    repo.delete(session.id.as_str()).await.expect("delete1");
    let gone = repo.find_by_id(session.id.as_str()).await.expect("find");
    assert!(gone.is_none());

    // Second delete is a no-op (idempotent).
    repo.delete(session.id.as_str()).await.expect("delete2");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn duplicate_create_returns_duplicate_key() {
    let (pool, _pg) = test_setup().await;
    let (tenant, user) = seed_tenant_user(&pool).await;
    let repo = PgSessionRepository::new(pool.clone());

    let session = fixture_session(&tenant, &user, "gpt-4o-mini");
    repo.create(&session).await.expect("first insert");
    let err = repo.create(&session).await.expect_err("dup");
    assert!(matches!(err, RepoError::DuplicateKey(_)));
}
