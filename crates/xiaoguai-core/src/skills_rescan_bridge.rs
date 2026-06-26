//! Phase 5 (skill-pack loader): production bridge for
//! `POST /v1/admin/skills/rescan`.
//!
//! Implements [`xiaoguai_api::skills_rescan::PackRescanner`] by closing over the
//! embedded `SQLite` pool + the live persona/team repositories and delegating to
//! [`crate::pack_runtime::scan_enabled_pack_agents`] — the same idempotent
//! conversational-team activation the boot scan runs. Lives in `xiaoguai-core`
//! (same layering as `skills_bridge.rs`): the api crate stays sqlx-free and
//! never reaches the `packs` runtime, so the hot-rescan capability is injected
//! as a trait object instead.
//!
//! Gated behind the `packs` feature — without it there is no pack runtime to
//! call, so `run_serve` leaves `AppState.pack_rescanner = None` and the route
//! returns 503.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::SqlitePool;
use xiaoguai_api::skills_rescan::{PackRescanError, PackRescanner};
use xiaoguai_personas::{PersonaRepository, TeamRepository};

/// Rescanner over the embedded `SQLite` pool and the live persona/team repos.
/// Cheap to clone (all `Arc`/pool handles).
#[derive(Clone)]
pub struct SqlitePackRescanner {
    pool: SqlitePool,
    personas: Arc<dyn PersonaRepository>,
    teams: Arc<dyn TeamRepository>,
}

impl SqlitePackRescanner {
    /// Build a rescanner. `personas`/`teams` MUST be the same repositories the
    /// running [`xiaoguai_api::AppState`] serves from, so a hot rescan upserts
    /// into the live stores `/orchestrate` reads.
    #[must_use]
    pub fn new(
        pool: SqlitePool,
        personas: Arc<dyn PersonaRepository>,
        teams: Arc<dyn TeamRepository>,
    ) -> Self {
        Self {
            pool,
            personas,
            teams,
        }
    }

    /// Convenience: box it as the trait object [`AppState`] holds.
    #[must_use]
    pub fn arc(
        pool: SqlitePool,
        personas: Arc<dyn PersonaRepository>,
        teams: Arc<dyn TeamRepository>,
    ) -> Arc<dyn PackRescanner> {
        Arc::new(Self::new(pool, personas, teams))
    }
}

#[async_trait]
impl PackRescanner for SqlitePackRescanner {
    async fn rescan(&self) -> Result<Vec<String>, PackRescanError> {
        crate::pack_runtime::scan_enabled_pack_agents(&self.pool, &*self.personas, &*self.teams)
            .await
            .map_err(|e| PackRescanError::Backend(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    /// Stand up the minimal `installed_skill_packs` table the scan reads.
    async fn installed_packs_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE installed_skill_packs (\
                id TEXT PRIMARY KEY, pack_slug TEXT NOT NULL, version TEXT NOT NULL, \
                config TEXT NOT NULL DEFAULT '{}', installed_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn app_store_reviews_pack_dir() -> String {
        format!(
            "{}/../../packs/app-store-reviews",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    async fn install_enabled(pool: &SqlitePool, slug: &str, pack_dir: &str) {
        let config = serde_json::json!({ "enabled": true, "pack_dir": pack_dir }).to_string();
        sqlx::query(
            "INSERT INTO installed_skill_packs (id, pack_slug, version, config) \
             VALUES (?, ?, '1.0.0', ?)",
        )
        .bind(slug)
        .bind(slug)
        .bind(&config)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn bridge_rescan_activates_conversational_team() {
        let pool = installed_packs_pool().await;
        install_enabled(&pool, "app-store-reviews", &app_store_reviews_pack_dir()).await;
        let personas: Arc<dyn PersonaRepository> =
            Arc::new(xiaoguai_personas::InMemoryPersonaRepository::new());
        let teams: Arc<dyn TeamRepository> =
            Arc::new(xiaoguai_personas::InMemoryTeamRepository::new());

        let rescanner = SqlitePackRescanner::new(pool.clone(), personas.clone(), teams.clone());
        let activated = rescanner.rescan().await.unwrap();
        assert_eq!(activated, vec!["app-store-reviews".to_string()]);

        // The team landed in the LIVE repos the route serves from.
        let ts = teams.list().await.unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].name, "app-store-reviews");
        assert!(personas.list().await.unwrap().len() >= 2);

        // …and the pack's config now records the agents → activation_status:active.
        let cfg: String = sqlx::query(
            "SELECT config FROM installed_skill_packs WHERE pack_slug = 'app-store-reviews'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("config")
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert!(v["agents"].as_array().is_some_and(|a| !a.is_empty()));
    }

    #[tokio::test]
    async fn bridge_rescan_is_idempotent() {
        let pool = installed_packs_pool().await;
        install_enabled(&pool, "app-store-reviews", &app_store_reviews_pack_dir()).await;
        let personas: Arc<dyn PersonaRepository> =
            Arc::new(xiaoguai_personas::InMemoryPersonaRepository::new());
        let teams: Arc<dyn TeamRepository> =
            Arc::new(xiaoguai_personas::InMemoryTeamRepository::new());
        let rescanner = SqlitePackRescanner::new(pool, personas.clone(), teams.clone());

        rescanner.rescan().await.unwrap();
        let (np, nt) = (
            personas.list().await.unwrap().len(),
            teams.list().await.unwrap().len(),
        );
        // A second hot rescan must not duplicate the personas or the team.
        rescanner.rescan().await.unwrap();
        assert_eq!(personas.list().await.unwrap().len(), np);
        assert_eq!(teams.list().await.unwrap().len(), nt);
    }
}
