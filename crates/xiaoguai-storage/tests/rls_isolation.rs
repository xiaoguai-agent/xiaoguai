//! v0.6.1 — End-to-end proof that RLS isolates tenants under a non-superuser
//! database role.
//!
//! The other repo integration tests run as the testcontainers `postgres`
//! superuser, which bypasses non-FORCE RLS policies. That suite verifies
//! *behaviour* but not the *security boundary*. This file fills the gap: it
//! provisions a `xiaoguai_app` role with no superuser bit, GRANTs the
//! minimum CRUD on RLS-enabled tables, and re-runs cross-tenant scenarios
//! through it. Without the per-request `SET LOCAL app.current_tenant_id`
//! the queries return empty result sets even though rows physically exist —
//! that is the production safety net.
//!
//! Marked `#[ignore]` like the other PG integration tests.

#![cfg(test)]

use chrono::Utc;
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use xiaoguai_storage::{
    db,
    repositories::{
        MessageRepository, PgMessageRepository, PgSessionRepository, SessionRepository,
    },
};
use xiaoguai_types::{
    ContentBlock, Message, MessageId, MessageRole, Session, SessionId, SessionStatus, TenantId,
    UserId,
};

/// Spin up a fresh PG container, run migrations, provision a non-superuser
/// role with table-level CRUD grants, and hand back both pools so callers
/// can seed data as superuser then exercise the boundary as the app role.
async fn setup_with_app_role() -> (PgPool, PgPool, ContainerAsync<Postgres>) {
    let pg = Postgres::default().start().await.expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let superuser_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let superuser_pool = db::connect(&superuser_url, 5).await.expect("connect su");
    db::migrate(&superuser_pool).await.expect("migrate");

    // Provision the application role and minimal grants. The role is NOT a
    // superuser, owns nothing, and therefore does not bypass RLS.
    sqlx::query("CREATE ROLE xiaoguai_app WITH LOGIN PASSWORD 'app'")
        .execute(&superuser_pool)
        .await
        .expect("create role");
    sqlx::query("GRANT USAGE ON SCHEMA public TO xiaoguai_app")
        .execute(&superuser_pool)
        .await
        .expect("grant schema");
    for table in [
        "tenants",
        "users",
        "user_roles",
        "sessions",
        "messages",
        "llm_providers",
        "mcp_servers",
        "token_usage",
    ] {
        sqlx::query(&format!(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON {table} TO xiaoguai_app"
        ))
        .execute(&superuser_pool)
        .await
        .unwrap_or_else(|e| panic!("grant {table}: {e}"));
    }
    // Some PG versions auto-create a sequence for SERIAL/BIGSERIAL columns;
    // token_usage has BIGSERIAL `id`. Grant USAGE so INSERTs can call
    // nextval() — harmless when no such sequence exists.
    let _ = sqlx::query("GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO xiaoguai_app")
        .execute(&superuser_pool)
        .await;

    let app_url = format!("postgres://xiaoguai_app:app@127.0.0.1:{port}/postgres");
    let app_pool = db::connect(&app_url, 5).await.expect("connect app");

    (superuser_pool, app_pool, pg)
}

async fn seed_tenant(pool: &PgPool, label: &str) -> (TenantId, UserId) {
    let tenant_id = TenantId::new();
    let user_id = UserId::new();
    sqlx::query("INSERT INTO tenants (id, name, display_name) VALUES ($1, $2, $3)")
        .bind(tenant_id.as_str())
        .bind(format!("ten-{label}-{}", tenant_id.as_str()))
        .bind(label)
        .execute(pool)
        .await
        .expect("seed tenant");
    sqlx::query(
        "INSERT INTO users (id, tenant_id, email, display_name)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id.as_str())
    .bind(tenant_id.as_str())
    .bind(format!("u-{label}@example.com"))
    .bind(format!("User {label}"))
    .execute(pool)
    .await
    .expect("seed user");
    (tenant_id, user_id)
}

fn fixture_session(tenant: &TenantId, user: &UserId, title: &str) -> Session {
    let now = Utc::now();
    Session {
        id: SessionId::new(),
        tenant_id: tenant.clone(),
        user_id: user.clone(),
        title: Some(title.into()),
        created_at: now,
        updated_at: now,
        model: "gpt-4o-mini".into(),
        status: SessionStatus::Active,
    }
}

fn fixture_message(session: &SessionId, text: &str) -> Message {
    Message {
        id: MessageId::new(),
        session_id: session.clone(),
        role: MessageRole::User,
        content: vec![ContentBlock::Text { text: text.into() }],
        created_at: Utc::now(),
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn app_role_sees_only_its_tenants_sessions() {
    let (su_pool, app_pool, _pg) = setup_with_app_role().await;

    // Seed two tenants + one session each as superuser (bypasses RLS).
    let (ten_a, user_a) = seed_tenant(&su_pool, "alpha").await;
    let (ten_b, user_b) = seed_tenant(&su_pool, "beta").await;
    let sess_a = fixture_session(&ten_a, &user_a, "alpha-session");
    let sess_b = fixture_session(&ten_b, &user_b, "beta-session");

    // We INSERT through the same superuser pool to avoid RLS gating on
    // INSERT (none of our policies have WITH CHECK clauses, but the
    // setup is cleaner this way).
    let su_session_repo = PgSessionRepository::new(su_pool.clone());
    // Superuser path: pass None — bypass works.
    su_session_repo.create(None, &sess_a).await.expect("a");
    su_session_repo.create(None, &sess_b).await.expect("b");

    // Now hit the same data via the app pool. Tenant A scope: should see
    // only A's row.
    let app_session_repo = PgSessionRepository::new(app_pool.clone());

    let a_view = app_session_repo
        .list_by_user(Some(ten_a.as_str()), user_a.as_str(), 100, 0)
        .await
        .expect("list a");
    assert_eq!(a_view.len(), 1, "tenant A should see exactly its own row");
    assert_eq!(a_view[0].id.as_str(), sess_a.id.as_str());

    // Tenant A trying to see tenant B's user should produce nothing —
    // RLS filters on the session's tenant, not the user_id.
    let cross = app_session_repo
        .list_by_user(Some(ten_a.as_str()), user_b.as_str(), 100, 0)
        .await
        .expect("list cross");
    assert!(cross.is_empty(), "tenant A must not see tenant B's rows");

    // Direct lookup of B's session under A's tenant scope → 404.
    let leaked = app_session_repo
        .find_by_id(Some(ten_a.as_str()), sess_b.id.as_str())
        .await
        .expect("find leaked");
    assert!(leaked.is_none(), "find_by_id leaked across tenants");

    // Same lookup under B's scope → visible.
    let visible = app_session_repo
        .find_by_id(Some(ten_b.as_str()), sess_b.id.as_str())
        .await
        .expect("find visible")
        .expect("present");
    assert_eq!(visible.id.as_str(), sess_b.id.as_str());

    // Without setting the GUC at all → empty for the app role.
    let unscoped = app_session_repo
        .find_by_id(None, sess_a.id.as_str())
        .await
        .expect("find unscoped");
    assert!(
        unscoped.is_none(),
        "unscoped app-role query must be empty — RLS denies access without GUC"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn app_role_cannot_read_other_tenants_messages() {
    let (su_pool, app_pool, _pg) = setup_with_app_role().await;
    let (ten_a, user_a) = seed_tenant(&su_pool, "alpha").await;
    let (ten_b, user_b) = seed_tenant(&su_pool, "beta").await;

    let sess_a = fixture_session(&ten_a, &user_a, "a");
    let sess_b = fixture_session(&ten_b, &user_b, "b");
    let su_session_repo = PgSessionRepository::new(su_pool.clone());
    su_session_repo.create(None, &sess_a).await.expect("sa");
    su_session_repo.create(None, &sess_b).await.expect("sb");

    let su_msg_repo = PgMessageRepository::new(su_pool.clone());
    let msg_a = fixture_message(&sess_a.id, "hello from alpha");
    let msg_b = fixture_message(&sess_b.id, "hello from beta");
    su_msg_repo.append(None, &msg_a).await.expect("ma");
    su_msg_repo.append(None, &msg_b).await.expect("mb");

    let app_msg_repo = PgMessageRepository::new(app_pool.clone());

    // Tenant A: sees its own message.
    let a_view = app_msg_repo
        .list_by_session(Some(ten_a.as_str()), sess_a.id.as_str(), 100, 0)
        .await
        .expect("list a");
    assert_eq!(a_view.len(), 1);
    assert_eq!(a_view[0].id.as_str(), msg_a.id.as_str());

    // Tenant A tries to peek into Tenant B's session — must be empty.
    let leak = app_msg_repo
        .list_by_session(Some(ten_a.as_str()), sess_b.id.as_str(), 100, 0)
        .await
        .expect("list cross");
    assert!(
        leak.is_empty(),
        "RLS on messages must hide rows whose session belongs to another tenant"
    );

    // Tenant B confirms its own row is still visible to it.
    let b_view = app_msg_repo
        .list_by_session(Some(ten_b.as_str()), sess_b.id.as_str(), 100, 0)
        .await
        .expect("list b");
    assert_eq!(b_view.len(), 1);
    assert_eq!(b_view[0].id.as_str(), msg_b.id.as_str());
}
