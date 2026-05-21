//! Casbin RBAC enforcement coverage.

use xiaoguai_auth::Authz;

async fn fixture() -> Authz {
    let authz = Authz::new_default().await.expect("load policy");
    // Wire some default user→role grants so we can also exercise the user→role
    // pathway. Tests that work directly with role names (sub = role) do not
    // need these grants.
    authz
        .grant_role("alice", "system_admin", "tenant-a")
        .await
        .unwrap();
    authz
        .grant_role("bob", "tenant_admin", "tenant-a")
        .await
        .unwrap();
    authz
        .grant_role("carol", "member", "tenant-a")
        .await
        .unwrap();
    authz
}

#[tokio::test]
async fn system_admin_can_do_anything() {
    let authz = fixture().await;
    for (res, act) in [
        ("/sessions/abc", "read"),
        ("/sessions/abc", "delete"),
        ("/audit/log", "read"),
        ("/billing/invoice", "write"),
        ("/anything/under/the/sun", "purge"),
    ] {
        assert!(
            authz
                .check("system_admin", "tenant-a", res, act)
                .await
                .unwrap(),
            "system_admin should pass {res} {act}"
        );
    }
}

#[tokio::test]
async fn tenant_admin_can_read_sessions() {
    let authz = fixture().await;
    assert!(authz
        .check("tenant_admin", "tenant-a", "/sessions/xyz", "read")
        .await
        .unwrap());
    assert!(authz
        .check("tenant_admin", "tenant-a", "/sessions/xyz", "delete")
        .await
        .unwrap());
}

#[tokio::test]
async fn tenant_admin_cannot_touch_arbitrary_root_paths() {
    let authz = fixture().await;
    assert!(
        !authz
            .check("tenant_admin", "tenant-a", "/secret/vault", "read")
            .await
            .unwrap(),
        "tenant_admin must not access unrelated resources"
    );
    assert!(
        !authz
            .check("tenant_admin", "tenant-a", "/billing/invoice", "write")
            .await
            .unwrap(),
        "tenant_admin can only read billing"
    );
}

#[tokio::test]
async fn member_only_on_own_sessions() {
    let authz = fixture().await;
    assert!(authz
        .check("member", "tenant-a", "/sessions/own/abc", "read")
        .await
        .unwrap());
    assert!(
        !authz
            .check("member", "tenant-a", "/sessions/other-user/abc", "read")
            .await
            .unwrap(),
        "member must not read foreign sessions"
    );
    assert!(
        !authz
            .check("member", "tenant-a", "/sessions/own/abc", "delete")
            .await
            .unwrap(),
        "member cannot delete even own sessions"
    );
}

#[tokio::test]
async fn cross_tenant_is_denied_for_explicit_user_grants() {
    let authz = fixture().await;
    // alice is system_admin in tenant-a; in tenant-b she has no grants.
    // When we ask using user_id as subject + tenant-b, the user→role
    // mapping is scoped per-tenant, so it must be denied.
    assert!(
        !authz
            .check("alice", "tenant-b", "/sessions/xyz", "read")
            .await
            .unwrap(),
        "alice has no role in tenant-b"
    );
    // But in tenant-a alice's user_id → system_admin grant resolves.
    assert!(authz
        .check("alice", "tenant-a", "/sessions/xyz", "read")
        .await
        .unwrap());
}

#[tokio::test]
async fn unknown_role_is_denied() {
    let authz = fixture().await;
    assert!(!authz
        .check("ghost_admin", "tenant-a", "/sessions/xyz", "read")
        .await
        .unwrap());
}

#[tokio::test]
async fn keymatch_wildcard_resource_path() {
    let authz = fixture().await;
    // Deep path under /sessions should match keyMatch(/sessions/*).
    assert!(authz
        .check("tenant_admin", "tenant-a", "/sessions/abc123", "read")
        .await
        .unwrap());
}

#[tokio::test]
async fn role_inheritance_system_admin_acts_as_tenant_admin() {
    let authz = fixture().await;
    // system_admin inherits tenant_admin which inherits member, so the
    // member-only resource must still resolve for system_admin.
    assert!(authz
        .check("system_admin", "tenant-a", "/sessions/own/abc", "read")
        .await
        .unwrap());
}

#[tokio::test]
async fn grant_role_via_runtime_api() {
    let authz = Authz::new_default().await.unwrap();
    // david has no grants yet.
    assert!(!authz
        .check("david", "tenant-x", "/sessions/foo", "read")
        .await
        .unwrap());
    authz
        .grant_role("david", "tenant_admin", "tenant-x")
        .await
        .unwrap();
    assert!(
        authz
            .check("david", "tenant-x", "/sessions/foo", "read")
            .await
            .unwrap(),
        "after granting tenant_admin, david should pass"
    );
}
