//! `SettingsRepository` — owner-level key/value preferences (DEC-033, single owner).
//!
//! A tiny kv over `app_settings` (migration 0040). First consumer is white-label
//! branding (the `branding` key, a JSON blob). Generic on purpose so future
//! runtime-editable preferences don't each need a bespoke table.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::repositories::error::RepoResult;

#[async_trait]
pub trait SettingsRepository: Send + Sync {
    /// Current value for `key`, or `None` when the key was never set.
    async fn get(&self, key: &str) -> RepoResult<Option<String>>;
    /// Upsert `key` to `value` (last write wins; refreshes `updated_at`).
    async fn set(&self, key: &str, value: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct SqliteSettingsRepository {
    pool: SqlitePool,
}

impl SqliteSettingsRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SettingsRepository for SqliteSettingsRepository {
    async fn get(&self, key: &str) -> RepoResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM app_settings WHERE key = ?1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(v,)| v))
    }

    async fn set(&self, key: &str, value: &str) -> RepoResult<()> {
        // Upsert: one row per key. `excluded` is the would-be-inserted row.
        sqlx::query(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET \
                 value = excluded.value, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect, migrate};

    /// Isolated temp-file DB per test — a bare `sqlite::memory:` URL is shared
    /// across the parallel test threads, so one test's writes leak into another.
    async fn fixture() -> (SqliteSettingsRepository, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let url = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("settings.db").display()
        );
        let p = connect(&url, 1).await.expect("connect");
        migrate(&p).await.expect("migrate");
        (SqliteSettingsRepository::new(p), dir)
    }

    #[tokio::test]
    async fn get_unset_key_is_none() {
        let (repo, _dir) = fixture().await;
        assert_eq!(repo.get("branding").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_then_get_round_trips() {
        let (repo, _dir) = fixture().await;
        repo.set("branding", r#"{"assistant_name":"Acme"}"#)
            .await
            .unwrap();
        assert_eq!(
            repo.get("branding").await.unwrap().as_deref(),
            Some(r#"{"assistant_name":"Acme"}"#)
        );
    }

    #[tokio::test]
    async fn set_is_idempotent_upsert_last_write_wins() {
        let (repo, _dir) = fixture().await;
        repo.set("branding", "first").await.unwrap();
        repo.set("branding", "second").await.unwrap();
        assert_eq!(
            repo.get("branding").await.unwrap().as_deref(),
            Some("second")
        );
        // Exactly one row — upsert, not insert.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM app_settings WHERE key = 'branding'")
            .fetch_one(repo.pool())
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    impl SqliteSettingsRepository {
        fn pool(&self) -> &SqlitePool {
            &self.pool
        }
    }
}
