//! sprint-14 S14-2: `HotlRedactionRepo` mutation methods + insert-only
//! revisions backed by migration 0028.
//!
//! Covers:
//! - Round-trip insert → read by id.
//! - `supersede_policy` creates a new revision row, deactivates prior, and
//!   links via `supersedes_policy_id`.
//! - `deactivate_policy` only flips `active = false`.
//! - Two concurrent identical INSERTs: one wins via the partial unique index
//!   (`WHERE active = true`); the loser gets `RepoError::DuplicateKey`.
//! - Two concurrent `supersede_policy(P1, ...)` calls: `FOR UPDATE` serialises
//!   them; the loser gets `RepoError::StaleRevision` carrying the winner's id.
//! - `get_revisions` walks the supersedes chain reverse-chronologically.
//! - Transaction rollback when the INSERT half fails (constraint violation
//!   injected mid-tx) leaves prior `active = true`.
//!
//! All tests `#[ignore]` per the crate convention (Docker required).

#![cfg(test)]

use sqlx::{Executor, PgPool, Row};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use uuid::Uuid;
use xiaoguai_storage::{
    db,
    repositories::{
        error::RepoError,
        hotl_redaction::{HotlRedactionRepo, PgHotlRedactionRepo, SupersedeFields},
    },
};

async fn start_pg() -> PgPool {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await
        .expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    // Leak the container — keep the DB alive for the duration of the test.
    std::mem::forget(pg);
    pool
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn insert_then_read_round_trip() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let row = repo
        .insert_policy(
            tenant,
            "tool_call.execute_python".into(),
            "$.password".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert");

    assert_eq!(row.tenant_id, tenant);
    assert_eq!(row.scope, "tool_call.execute_python");
    assert_eq!(row.jsonpath, "$.password");
    assert_eq!(row.applies_to, vec!["sse".to_string()]);
    assert!(row.active);
    assert_eq!(row.created_by, "alice");
    assert_eq!(row.supersedes_policy_id, None);

    // Repo-side read via load_for_tenant only returns active rows (S14-3
    // contract); raw count via SQL confirms the row exists.
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM hotl_redaction_policies WHERE id = $1")
            .bind(row.id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count.0, 1);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn supersede_creates_new_revision() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let p1 = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.password".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert p1");

    let p2 = repo
        .supersede_policy(
            p1.id,
            SupersedeFields {
                tenant_id: tenant,
                scope: "tool_call.foo".into(),
                jsonpath: "$.password_v2".into(),
                applies_to: vec!["sse".into(), "audit".into()],
            },
            "bob".into(),
        )
        .await
        .expect("supersede p1");

    assert_eq!(p2.supersedes_policy_id, Some(p1.id));
    assert!(p2.active, "new revision must be active");
    assert_eq!(p2.jsonpath, "$.password_v2");
    assert_eq!(p2.created_by, "bob");

    // Prior must be deactivated.
    let p1_active: (bool,) =
        sqlx::query_as("SELECT active FROM hotl_redaction_policies WHERE id = $1")
            .bind(p1.id)
            .fetch_one(&pool)
            .await
            .expect("query p1.active");
    assert!(!p1_active.0, "p1 must be deactivated after supersede");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn deactivate_only_sets_active_false() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let row = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.x".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert");

    repo.deactivate_policy(row.id, "alice".into())
        .await
        .expect("deactivate");

    // Row still present — only `active` flipped.
    let r: sqlx::postgres::PgRow =
        sqlx::query("SELECT id, active FROM hotl_redaction_policies WHERE id = $1")
            .bind(row.id)
            .fetch_one(&pool)
            .await
            .expect("query");
    let active: bool = r.get("active");
    assert!(!active, "deactivate must flip active=false");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn concurrent_identical_inserts_one_fails() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let repo_a = repo.clone();
    let repo_b = repo.clone();
    let scope = "tool_call.foo".to_string();
    let jsonpath = "$.x".to_string();

    let scope_a = scope.clone();
    let jsonpath_a = jsonpath.clone();
    let h1 = tokio::spawn(async move {
        repo_a
            .insert_policy(
                tenant,
                scope_a,
                jsonpath_a,
                vec!["sse".into()],
                "alice".into(),
            )
            .await
    });
    let h2 = tokio::spawn(async move {
        repo_b
            .insert_policy(tenant, scope, jsonpath, vec!["sse".into()], "alice".into())
            .await
    });

    let r1 = h1.await.expect("join h1");
    let r2 = h2.await.expect("join h2");

    let (ok_count, dup_count) = match (&r1, &r2) {
        (Ok(_), Err(RepoError::DuplicateKey(_))) | (Err(RepoError::DuplicateKey(_)), Ok(_)) => {
            (1, 1)
        }
        // Both Ok would mean the partial unique index isn't active — fail.
        (Ok(_), Ok(_)) => panic!(
            "both inserts succeeded; partial unique index on (tenant,scope,jsonpath) WHERE active=true is missing or broken"
        ),
        other => panic!("unexpected result pair: {other:?}"),
    };
    assert_eq!(ok_count, 1);
    assert_eq!(dup_count, 1);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn concurrent_supersedes_against_same_prior() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let p1 = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.x".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert p1");

    let repo_a = repo.clone();
    let repo_b = repo.clone();
    let prior_id = p1.id;

    let h1 = tokio::spawn(async move {
        repo_a
            .supersede_policy(
                prior_id,
                SupersedeFields {
                    tenant_id: tenant,
                    scope: "tool_call.foo".into(),
                    jsonpath: "$.a".into(),
                    applies_to: vec!["sse".into()],
                },
                "bob".into(),
            )
            .await
    });
    let h2 = tokio::spawn(async move {
        repo_b
            .supersede_policy(
                prior_id,
                SupersedeFields {
                    tenant_id: tenant,
                    scope: "tool_call.foo".into(),
                    jsonpath: "$.b".into(),
                    applies_to: vec!["sse".into()],
                },
                "carol".into(),
            )
            .await
    });

    let r1 = h1.await.expect("join h1");
    let r2 = h2.await.expect("join h2");

    // Exactly one Ok, exactly one StaleRevision pointing at the winner.
    let (winner_id, loser_head) = match (r1, r2) {
        (Ok(new), Err(RepoError::StaleRevision { current_head_id })) => (new.id, current_head_id),
        (Err(RepoError::StaleRevision { current_head_id }), Ok(new)) => (new.id, current_head_id),
        other => panic!(
            "expected one Ok + one StaleRevision, got {other:?}; FOR UPDATE row lock did not serialise the supersedes"
        ),
    };
    assert_eq!(
        loser_head, winner_id,
        "StaleRevision must carry the winner's new-head id"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn get_revisions_walks_chain() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let p1 = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.a".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert p1");

    let p2 = repo
        .supersede_policy(
            p1.id,
            SupersedeFields {
                tenant_id: tenant,
                scope: "tool_call.foo".into(),
                jsonpath: "$.b".into(),
                applies_to: vec!["sse".into()],
            },
            "bob".into(),
        )
        .await
        .expect("supersede p1");

    let p3 = repo
        .supersede_policy(
            p2.id,
            SupersedeFields {
                tenant_id: tenant,
                scope: "tool_call.foo".into(),
                jsonpath: "$.c".into(),
                applies_to: vec!["sse".into()],
            },
            "carol".into(),
        )
        .await
        .expect("supersede p2");

    // get_revisions takes any id in the chain and returns the full chain.
    let revs = repo.get_revisions(p3.id).await.expect("get_revisions");
    assert_eq!(revs.len(), 3, "expected 3 rows in chain");
    // Reverse-chronological: newest first.
    assert_eq!(revs[0].id, p3.id);
    assert_eq!(revs[1].id, p2.id);
    assert_eq!(revs[2].id, p1.id);

    // Asking via the middle id should also yield the full chain.
    let revs_mid = repo.get_revisions(p2.id).await.expect("get_revisions mid");
    assert_eq!(revs_mid.len(), 3);
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn tx_failure_rolls_back_atomically() {
    // Force the INSERT half of supersede_policy to fail by violating
    // a FK / NOT NULL the repo doesn't sanitise. The cleanest way that
    // doesn't depend on internals: pre-insert an active sibling row with
    // the same (tenant, scope, jsonpath) so the partial unique index
    // fires on the INSERT. After the failed call, the prior row must still
    // be active (UPDATE was rolled back together with the failed INSERT).
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let p1 = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.original".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert p1");

    // Sibling that occupies the (tenant, scope, "$.collide") slot.
    let _collider = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.collide".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert collider");

    // Now try to supersede p1 with the same (tenant, scope, jsonpath) as
    // the collider. UPDATE-prior-active=false runs first, then INSERT
    // fires the partial unique index → tx rolled back → p1 stays active.
    let err = repo
        .supersede_policy(
            p1.id,
            SupersedeFields {
                tenant_id: tenant,
                scope: "tool_call.foo".into(),
                jsonpath: "$.collide".into(),
                applies_to: vec!["sse".into()],
            },
            "bob".into(),
        )
        .await
        .expect_err("supersede must fail on partial-unique collision");
    assert!(
        matches!(err, RepoError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );

    let p1_active: (bool,) =
        sqlx::query_as("SELECT active FROM hotl_redaction_policies WHERE id = $1")
            .bind(p1.id)
            .fetch_one(&pool)
            .await
            .expect("p1 still queryable");
    assert!(
        p1_active.0,
        "p1.active must remain true — UPDATE was rolled back together with the failed INSERT"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn stale_revision_on_already_deactivated_prior() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());
    let tenant = Uuid::new_v4();

    let p1 = repo
        .insert_policy(
            tenant,
            "tool_call.foo".into(),
            "$.a".into(),
            vec!["sse".into()],
            "alice".into(),
        )
        .await
        .expect("insert p1");

    repo.deactivate_policy(p1.id, "alice".into())
        .await
        .expect("deactivate p1");

    let err = repo
        .supersede_policy(
            p1.id,
            SupersedeFields {
                tenant_id: tenant,
                scope: "tool_call.foo".into(),
                jsonpath: "$.b".into(),
                applies_to: vec!["sse".into()],
            },
            "bob".into(),
        )
        .await
        .expect_err("supersede on inactive prior must fail");
    assert!(
        matches!(err, RepoError::StaleRevision { .. }),
        "expected StaleRevision, got {err:?}"
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn tenant_settings_redaction_required_default_false() {
    // Migration 0028 adds `redaction_policy_required boolean NOT NULL
    // DEFAULT false` to `tenant_settings`. Sanity-check the column is
    // there and defaults correctly.
    let pool = start_pg().await;
    // Insert a tenant row (FK target).
    pool.execute("INSERT INTO tenants (id, name, display_name) VALUES ('t1', 't1', 't1')")
        .await
        .expect("seed tenant");
    pool.execute("INSERT INTO tenant_settings (tenant_id) VALUES ('t1')")
        .await
        .expect("seed tenant_settings");

    let val: (bool,) = sqlx::query_as(
        "SELECT redaction_policy_required FROM tenant_settings WHERE tenant_id = $1",
    )
    .bind("t1")
    .fetch_one(&pool)
    .await
    .expect("query column");
    assert!(!val.0, "redaction_policy_required must default to false");
}
