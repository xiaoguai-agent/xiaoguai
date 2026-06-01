//! sprint-14 S14-2: pre-flight smoke for migration 0028.
//!
//! Seeds the post-0027 snapshot — a tenant + a few `hotl_redaction_policies`
//! rows — **before** 0028 is applied, then runs the migration and asserts:
//!
//! 1. Zero partial-unique-index violations (the seed must not contain two
//!    rows with the same `(tenant, scope, jsonpath)` once 0028 lands).
//! 2. The seeded rows all carry `active = true` (the column default backfills
//!    correctly).
//! 3. `created_by` is `'system'` for backfilled rows (per the migration
//!    default).
//! 4. The partial unique index is actually in place — a duplicate INSERT
//!    fails with 23505.
//! 5. The `tenant_settings.redaction_policy_required` column exists and
//!    defaults to false.
//!
//! ## Seed strategy
//!
//! `sqlx::migrate!` always runs the full migration set in order — there is
//! no public API to stop before 0028. We work around this by running the
//! migrator twice on the same database with the seed in between is **not**
//! possible. Instead, we use a custom migrator that walks the migrations
//! directory manually, runs each up through 0027, seeds, then applies 0028.
//!
//! Marked `#[ignore]` — Docker required, same convention as the rest of the
//! crate's testcontainers-backed tests.

#![cfg(test)]

use std::path::PathBuf;

use chrono::Utc;
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use uuid::Uuid;
use xiaoguai_storage::db;

/// Apply every `crates/xiaoguai-storage/migrations/*.sql` file up to and
/// including the cut-off (inclusive), in lexicographic order. Skips files
/// strictly after the cut-off. `sqlx::migrate!` doesn't expose a "stop
/// here" hook, so we read the directory ourselves.
async fn apply_migrations_through(pool: &PgPool, cutoff_prefix: &str) -> anyhow::Result<()> {
    let mut dir: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    dir.push("migrations");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();

    for path in entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        // Filenames are `NNNN_name.sql`; the 4-digit prefix is the sort key.
        let prefix = stem.split('_').next().unwrap_or("");
        if prefix > cutoff_prefix {
            break;
        }
        let sql = std::fs::read_to_string(&path)?;
        // Some migration files contain `$$` PL/pgSQL bodies; sqlx's raw
        // execute handles them. All our migrations split by ';' but
        // running the whole file in one call is safer.
        sqlx::raw_sql(&sql).execute(pool).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn migration_0028_applies_cleanly_on_post_0027_snapshot() {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await
        .expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    std::mem::forget(pg);

    // ---- 1. Run every migration up through 0027. -------------------------
    apply_migrations_through(&pool, "0027")
        .await
        .expect("apply 0001..0027");

    // ---- 2. Seed three v1.10.x-shape redaction policies. -----------------
    let tenant = Uuid::new_v4();
    for (scope, jsonpath) in [
        ("tool_call.execute_python", "$.password"),
        ("tool_call.http_get", "$.headers.authorization"),
        ("*", "$.token"),
    ] {
        sqlx::query(
            "INSERT INTO hotl_redaction_policies \
             (id, tenant_id, scope, jsonpath, applies_to, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(tenant)
        .bind(scope)
        .bind(jsonpath)
        .bind(vec!["sse".to_string()])
        .bind(Utc::now())
        .execute(&pool)
        .await
        .expect("seed policy");
    }

    // ---- 3. Apply 0028. -------------------------------------------------
    apply_migrations_through(&pool, "0028")
        .await
        .expect("apply 0028 — partial-unique-index violation on seed?");

    // ---- 4. Backfilled columns have the right defaults. -----------------
    let backfilled: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM hotl_redaction_policies \
         WHERE active = TRUE AND created_by = 'system' AND supersedes_policy_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("count backfilled");
    assert_eq!(
        backfilled.0, 3,
        "all 3 seeded rows must be active + 'system' + no prior"
    );

    // ---- 5. Partial unique index is enforced. ---------------------------
    let dup = sqlx::query(
        "INSERT INTO hotl_redaction_policies \
         (id, tenant_id, scope, jsonpath, applies_to, created_by, active) \
         VALUES ($1, $2, 'tool_call.execute_python', '$.password', \
                 ARRAY['sse']::TEXT[], 'tester', TRUE)",
    )
    .bind(Uuid::new_v4())
    .bind(tenant)
    .execute(&pool)
    .await;
    assert!(
        dup.is_err(),
        "duplicate active (tenant, scope, jsonpath) must violate the partial unique index"
    );

    // ---- 6. tenant_settings has the new column with the right default. --
    pool.execute_unprepared(
        "INSERT INTO tenants (id, name, display_name) VALUES ('preflight-tenant', 'preflight-tenant', 'preflight-tenant')",
    )
    .await
    .expect("seed tenants row");
    pool.execute_unprepared("INSERT INTO tenant_settings (tenant_id) VALUES ('preflight-tenant')")
        .await
        .expect("seed tenant_settings row");
    let val: (bool,) = sqlx::query_as(
        "SELECT redaction_policy_required FROM tenant_settings WHERE tenant_id = 'preflight-tenant'",
    )
    .fetch_one(&pool)
    .await
    .expect("query column");
    assert!(!val.0, "redaction_policy_required must default to false");
}

// Local re-export for `execute_unprepared` (introduced in sqlx 0.8); avoid
// an additional `use` in the test body by going through a free function.
trait PoolExt {
    async fn execute_unprepared(&self, sql: &str) -> Result<(), sqlx::Error>;
}

impl PoolExt for PgPool {
    async fn execute_unprepared(&self, sql: &str) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(sql).execute(self).await?;
        Ok(())
    }
}
