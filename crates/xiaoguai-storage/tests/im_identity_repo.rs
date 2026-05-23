//! v0.7.3: Integration tests for `PgImIdentityRepository`.
//!
//! Boots a real Postgres + applies migrations + exercises the
//! `resolve_or_create_*` helpers. Tests are `#[ignore = "requires
//! Docker"]` so the fast path stays clean; run with
//! `cargo test -p xiaoguai-storage --test im_identity_repo -- --ignored`.

mod common;

use xiaoguai_storage::repositories::{
    ExternalConversation, ExternalIdentity, ImIdentityRepository, PgImIdentityRepository,
};

#[tokio::test]
#[ignore = "requires Docker"]
async fn first_webhook_auto_creates_tenant_user_and_mapping() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgImIdentityRepository::new(pool.clone());

    let identity = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_x",
                user_external_id: "ou_alice",
            },
            Some("Alice"),
        )
        .await
        .expect("first resolve");

    // Mapping row exists.
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM im_identities")
        .fetch_one(&pool)
        .await
        .expect("count im_identities");
    assert_eq!(count, 1);

    // Tenant row exists with the synthetic name.
    let (tname,): (String,) = sqlx::query_as("SELECT name FROM tenants WHERE id = $1")
        .bind(&identity.tenant_id)
        .fetch_one(&pool)
        .await
        .expect("tenant lookup");
    assert_eq!(tname, "im:feishu:ten_x");

    // User row exists with the synthetic email + display hint.
    let (uemail, udisplay): (String, String) =
        sqlx::query_as("SELECT email, display_name FROM users WHERE id = $1")
            .bind(&identity.user_id)
            .fetch_one(&pool)
            .await
            .expect("user lookup");
    assert_eq!(uemail, "ou_alice@ten_x.feishu.im.invalid");
    assert_eq!(udisplay, "Alice");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn second_webhook_for_same_identity_reuses_rows() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgImIdentityRepository::new(pool.clone());

    let a = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_x",
                user_external_id: "ou_alice",
            },
            None,
        )
        .await
        .expect("first");
    let b = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_x",
                user_external_id: "ou_alice",
            },
            None,
        )
        .await
        .expect("second");

    assert_eq!(a, b, "identical webhook should reuse the same identity");

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM tenants")
        .fetch_one(&pool)
        .await
        .expect("count tenants");
    assert_eq!(count, 1, "tenant row should not be duplicated");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn conversations_are_per_tenant_external_pair() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgImIdentityRepository::new(pool.clone());

    let identity = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_x",
                user_external_id: "ou_alice",
            },
            None,
        )
        .await
        .expect("identity");

    let c1 = repo
        .resolve_or_create_conversation(
            ExternalConversation {
                provider: "feishu",
                tenant_external_id: "ten_x",
                conversation_id: "oc_a",
            },
            &identity,
            None,
        )
        .await
        .expect("c1");
    let c2 = repo
        .resolve_or_create_conversation(
            ExternalConversation {
                provider: "feishu",
                tenant_external_id: "ten_x",
                conversation_id: "oc_b",
            },
            &identity,
            None,
        )
        .await
        .expect("c2");
    let c1_again = repo
        .resolve_or_create_conversation(
            ExternalConversation {
                provider: "feishu",
                tenant_external_id: "ten_x",
                conversation_id: "oc_a",
            },
            &identity,
            None,
        )
        .await
        .expect("c1 again");

    assert_ne!(c1.session_id, c2.session_id);
    assert_eq!(c1.session_id, c1_again.session_id);

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count sessions");
    assert_eq!(count, 2);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn different_tenant_externals_produce_isolated_tenants() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgImIdentityRepository::new(pool.clone());

    let x = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_x",
                user_external_id: "ou_alice",
            },
            None,
        )
        .await
        .expect("x");
    let y = repo
        .resolve_or_create_identity(
            ExternalIdentity {
                provider: "feishu",
                tenant_external_id: "ten_y",
                user_external_id: "ou_alice",
            },
            None,
        )
        .await
        .expect("y");

    assert_ne!(x.tenant_id, y.tenant_id);
    assert_ne!(
        x.user_id, y.user_id,
        "users are scoped per tenant_external_id even when user_external_id matches"
    );
}
