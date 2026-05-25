//! v1.2.28 — PG-backed `SkillPackRepository`.
//!
//! Table: `installed_skill_packs` (migration 0015).
//!
//! The `UNIQUE (tenant_id, pack_slug)` constraint fires on duplicate
//! installs; we surface that as `SkillPackError::AlreadyInstalled` by
//! inspecting the sqlx error code (PG 23505 — `unique_violation`).
//!
//! Lives in `xiaoguai-core` (same layering pattern as `audit_bridge.rs`):
//! the api crate stays sqlx-free; SQL lives here.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use xiaoguai_api::skills::{InstalledPackRow, SkillPackError, SkillPackRepository};

// ── struct ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PgSkillPackRepository {
    pool: PgPool,
}

impl PgSkillPackRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<dyn SkillPackRepository> {
        Arc::new(Self::new(pool))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn pg_err(e: sqlx::Error) -> SkillPackError {
    SkillPackError::Backend(e.to_string())
}

/// Detect PG `unique_violation` (SQLSTATE 23505).
fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = e {
        // sqlx exposes the SQLSTATE code via `code()`.
        return db.code().as_deref() == Some("23505");
    }
    false
}

// ── DB row shape ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct PackRow {
    id: String,
    tenant_id: String,
    pack_slug: String,
    version: String,
    config: serde_json::Value,
    installed_at: chrono::DateTime<chrono::Utc>,
}

impl From<PackRow> for InstalledPackRow {
    fn from(r: PackRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            pack_slug: r.pack_slug,
            version: r.version,
            config: r.config,
            installed_at: r.installed_at,
        }
    }
}

// ── trait impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl SkillPackRepository for PgSkillPackRepository {
    async fn list(&self, tenant_id: &str) -> Result<Vec<InstalledPackRow>, SkillPackError> {
        let rows: Vec<PackRow> = sqlx::query_as(
            "SELECT id::TEXT, tenant_id::TEXT, pack_slug, version, config, installed_at \
             FROM installed_skill_packs \
             WHERE tenant_id = $1::UUID \
             ORDER BY installed_at ASC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(pg_err)?;

        Ok(rows.into_iter().map(InstalledPackRow::from).collect())
    }

    async fn install(&self, row: InstalledPackRow) -> Result<InstalledPackRow, SkillPackError> {
        let result = sqlx::query(
            "INSERT INTO installed_skill_packs \
                (id, tenant_id, pack_slug, version, config, installed_at) \
             VALUES ($1::UUID, $2::UUID, $3, $4, $5, $6)",
        )
        .bind(&row.id)
        .bind(&row.tenant_id)
        .bind(&row.pack_slug)
        .bind(&row.version)
        .bind(&row.config)
        .bind(row.installed_at)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(row),
            Err(e) if is_unique_violation(&e) => Err(SkillPackError::AlreadyInstalled),
            Err(e) => Err(pg_err(e)),
        }
    }

    async fn uninstall(&self, id: &str) -> Result<(), SkillPackError> {
        let result = sqlx::query("DELETE FROM installed_skill_packs WHERE id = $1::UUID")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(pg_err)?;

        if result.rows_affected() == 0 {
            return Err(SkillPackError::NotFound);
        }
        Ok(())
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::Error as SqlxError;
    use uuid::Uuid;

    // ── unit tests ────────────────────────────────────────────────────────────

    #[test]
    fn is_unique_violation_returns_false_for_other_errors() {
        // Can't construct a DatabaseError directly in unit tests, but we can
        // confirm the helper doesn't panic on a RowNotFound variant.
        let e = SqlxError::RowNotFound;
        assert!(!is_unique_violation(&e));
    }

    #[test]
    fn pack_row_converts_to_installed_pack_row() {
        let now = Utc::now();
        let pr = PackRow {
            id: "id-1".into(),
            tenant_id: "tenant-1".into(),
            pack_slug: "rag-hr".into(),
            version: "1.0.0".into(),
            config: serde_json::json!({"top_k": 5}),
            installed_at: now,
        };
        let row: InstalledPackRow = pr.into();
        assert_eq!(row.id, "id-1");
        assert_eq!(row.pack_slug, "rag-hr");
        assert_eq!(row.config["top_k"], 5);
        assert_eq!(row.installed_at, now);
    }

    // ── PG integration tests ──────────────────────────────────────────────────
    // Run with: DATABASE_URL=postgres://... cargo test -p xiaoguai-core
    //           --ignore-rust-version -- --ignored skills_pg_

    async fn pg_pool() -> sqlx::PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG bridge tests");
        sqlx::PgPool::connect(&url).await.expect("pg connect")
    }

    fn make_row(tenant_id: &str, slug: &str) -> InstalledPackRow {
        InstalledPackRow {
            id: Uuid::new_v4().to_string(),
            tenant_id: tenant_id.to_string(),
            pack_slug: slug.to_string(),
            version: "1.0.0".into(),
            config: serde_json::json!({"top_k": 10}),
            installed_at: Utc::now(),
        }
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skills_pg_install_list_uninstall() {
        let pool = pg_pool().await;
        let repo = PgSkillPackRepository::new(pool);
        let tid = Uuid::new_v4().to_string();

        let row = make_row(&tid, "rag-hr");
        let saved = repo.install(row.clone()).await.unwrap();
        assert_eq!(saved.pack_slug, "rag-hr");

        let listed = repo.list(&tid).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, row.id);

        repo.uninstall(&saved.id).await.unwrap();
        let after = repo.list(&tid).await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skills_pg_duplicate_install_returns_already_installed() {
        let pool = pg_pool().await;
        let repo = PgSkillPackRepository::new(pool);
        let tid = Uuid::new_v4().to_string();

        repo.install(make_row(&tid, "pr-review")).await.unwrap();
        let err = repo.install(make_row(&tid, "pr-review")).await.unwrap_err();
        assert!(
            matches!(err, SkillPackError::AlreadyInstalled),
            "second install must be AlreadyInstalled: {err:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skills_pg_uninstall_missing_is_not_found() {
        let pool = pg_pool().await;
        let repo = PgSkillPackRepository::new(pool);
        let err = repo.uninstall(&Uuid::new_v4().to_string()).await.unwrap_err();
        assert!(matches!(err, SkillPackError::NotFound));
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skills_pg_list_scopes_by_tenant() {
        let pool = pg_pool().await;
        let repo = PgSkillPackRepository::new(pool);
        let tid_a = Uuid::new_v4().to_string();
        let tid_b = Uuid::new_v4().to_string();

        repo.install(make_row(&tid_a, "rag-legal")).await.unwrap();
        repo.install(make_row(&tid_a, "rag-finance")).await.unwrap();
        repo.install(make_row(&tid_b, "rag-hr")).await.unwrap();

        let a = repo.list(&tid_a).await.unwrap();
        assert_eq!(a.len(), 2);
        let b = repo.list(&tid_b).await.unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].pack_slug, "rag-hr");
        let c = repo.list(&Uuid::new_v4().to_string()).await.unwrap();
        assert!(c.is_empty());
    }
}
