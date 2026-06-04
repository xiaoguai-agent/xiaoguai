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
use sqlx::SqlitePool;
use xiaoguai_api::skills::{InstalledPackRow, SkillPackError, SkillPackRepository};

// ── struct ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PgSkillPackRepository {
    pool: SqlitePool,
}

impl PgSkillPackRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<dyn SkillPackRepository> {
        Arc::new(Self::new(pool))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn pg_err(e: sqlx::Error) -> SkillPackError {
    SkillPackError::Backend(e.to_string())
}

/// Detect a `SQLite` `UNIQUE constraint failed` violation.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = e {
        return db.message().contains("UNIQUE constraint failed");
    }
    false
}

// ── DB row shape ──────────────────────────────────────────────────────────────
//
// DEC-033: `installed_skill_packs.tenant_id` column dropped. The domain
// `InstalledPackRow` keeps a `tenant_id: String` field, synthesized to the
// owner id on read.

#[derive(sqlx::FromRow)]
struct PackRow {
    id: String,
    pack_slug: String,
    version: String,
    config: serde_json::Value,
    installed_at: chrono::DateTime<chrono::Utc>,
}

impl From<PackRow> for InstalledPackRow {
    fn from(r: PackRow) -> Self {
        Self {
            id: r.id,
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
    async fn list(&self) -> Result<Vec<InstalledPackRow>, SkillPackError> {
        let rows: Vec<PackRow> = sqlx::query_as(
            "SELECT id, pack_slug, version, config, installed_at \
             FROM installed_skill_packs \
             ORDER BY installed_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(pg_err)?;

        Ok(rows.into_iter().map(InstalledPackRow::from).collect())
    }

    async fn install(&self, row: InstalledPackRow) -> Result<InstalledPackRow, SkillPackError> {
        // DEC-033: tenant_id column dropped from installed_skill_packs.
        let result = sqlx::query(
            "INSERT INTO installed_skill_packs \
                (id, pack_slug, version, config, installed_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
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
        let result = sqlx::query("DELETE FROM installed_skill_packs WHERE id = ?")
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
        // DEC-033: PackRow no longer carries tenant_id; the conversion
        // synthesizes the single owner id.
        let pr = PackRow {
            id: "id-1".into(),
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

    // ── SQLite integration tests (DEC-033) ────────────────────────────────────

    async fn sqlite_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
            .await
            .unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();
        (dir, pool)
    }

    fn make_row(slug: &str) -> InstalledPackRow {
        InstalledPackRow {
            id: Uuid::new_v4().to_string(),
            pack_slug: slug.to_string(),
            version: "1.0.0".into(),
            config: serde_json::json!({"top_k": 10}),
            installed_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn skills_install_list_uninstall() {
        let (_dir, pool) = sqlite_pool().await;
        let repo = PgSkillPackRepository::new(pool);

        let row = make_row("rag-hr");
        let saved = repo.install(row.clone()).await.unwrap();
        assert_eq!(saved.pack_slug, "rag-hr");

        let listed = repo.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, row.id);

        repo.uninstall(&saved.id).await.unwrap();
        let after = repo.list().await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn skills_duplicate_install_returns_already_installed() {
        let (_dir, pool) = sqlite_pool().await;
        let repo = PgSkillPackRepository::new(pool);

        repo.install(make_row("pr-review")).await.unwrap();
        // DEC-033: UNIQUE constraint is now on `pack_slug` alone.
        let err = repo.install(make_row("pr-review")).await.unwrap_err();
        assert!(
            matches!(err, SkillPackError::AlreadyInstalled),
            "second install must be AlreadyInstalled: {err:?}"
        );
    }

    #[tokio::test]
    async fn skills_uninstall_missing_is_not_found() {
        let (_dir, pool) = sqlite_pool().await;
        let repo = PgSkillPackRepository::new(pool);
        let err = repo
            .uninstall(&Uuid::new_v4().to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, SkillPackError::NotFound));
    }

    // DELETED skills_pg_list_scopes_by_tenant: under DEC-033 there is one
    // implicit owner and `list` returns all rows regardless of tenant_id, so
    // per-tenant isolation is no longer a meaningful behaviour to assert.
}
