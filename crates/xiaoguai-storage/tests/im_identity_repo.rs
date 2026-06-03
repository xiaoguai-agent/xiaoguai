//! v0.7.3: Integration tests for `PgImIdentityRepository` (`SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`
//! and exercises the `resolve_or_create_*` helpers. Under the single-user pivot
//! there is no `tenants` table; the resolved `tenant_id` is always
//! `OWNER_TENANT_ID`, and users are keyed by their synthetic email.

mod common;

use common::test_setup;
use xiaoguai_storage::repositories::{
    ExternalConversation, ExternalIdentity, ImIdentityRepository, PgImIdentityRepository,
};
use xiaoguai_storage::OWNER_TENANT_ID;

#[tokio::test]
async fn first_webhook_auto_creates_user_and_mapping() {
    let (pool, _guard) = test_setup().await;
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

    // Single owner under the pivot.
    assert_eq!(identity.tenant_id, OWNER_TENANT_ID);

    // Mapping row exists.
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM im_identities")
        .fetch_one(&pool)
        .await
        .expect("count im_identities");
    assert_eq!(count, 1);

    // User row exists with the synthetic email + display hint.
    let (uemail, udisplay): (String, String) =
        sqlx::query_as("SELECT email, display_name FROM users WHERE id = ?")
            .bind(&identity.user_id)
            .fetch_one(&pool)
            .await
            .expect("user lookup");
    assert_eq!(uemail, "ou_alice@ten_x.feishu.im.invalid");
    assert_eq!(udisplay, "Alice");
}

#[tokio::test]
async fn second_webhook_for_same_identity_reuses_rows() {
    let (pool, _guard) = test_setup().await;
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

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM im_identities")
        .fetch_one(&pool)
        .await
        .expect("count im_identities");
    assert_eq!(count, 1, "identity row should not be duplicated");
}

#[tokio::test]
async fn conversations_are_per_external_pair() {
    let (pool, _guard) = test_setup().await;
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
async fn different_tenant_externals_produce_distinct_users() {
    let (pool, _guard) = test_setup().await;
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

    // Both resolve to the single owner tenant, but the synthetic email encodes
    // tenant_external_id, so the users (and mappings) stay distinct.
    assert_eq!(x.tenant_id, OWNER_TENANT_ID);
    assert_eq!(y.tenant_id, OWNER_TENANT_ID);
    assert_ne!(
        x.user_id, y.user_id,
        "users are distinct per tenant_external_id even when user_external_id matches"
    );
}
