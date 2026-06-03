//! Sprint-8 S8-7 (DEC-023.3): Postgres-backed implementations of the
//! `SkillProposalRepository` and `TenantSettingsReader` seams declared
//! in [`crate::skill_author`].
//!
//! Tables come from migration `0021_skill_proposals.sql`:
//!   * `skill_proposals(id TEXT PK, tenant_id, proposed_by, name, version,
//!                      manifest_json JSONB, status, reason,
//!                      created_at, decided_at, decided_by)`
//!   * `tenant_settings(tenant_id TEXT PK, settings JSONB, updated_at)`
//!
//! The `HotL` gate + audit-sink adapters live in
//! `xiaoguai-core::skill_author_bridge` so we don't pull an
//! `xiaoguai-api`/`xiaoguai-audit` dependency cycle back into this crate.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::SqlitePool;

use crate::skill_author::{
    ProposalRow, ProposalStatus, SkillAuthorError, SkillManifest, SkillProposalRepository,
    TenantSettingsReader,
};

// ---------------------------------------------------------------------------
// Row shapes
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct ProposalDbRow {
    id: String,
    proposed_by: String,
    manifest_json: sqlx::types::Json<JsonValue>,
    status: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
    decided_by: Option<String>,
}

impl ProposalDbRow {
    fn try_into_domain(self) -> Result<ProposalRow, SkillAuthorError> {
        let manifest: SkillManifest = serde_json::from_value(self.manifest_json.0)
            .map_err(|e| SkillAuthorError::Backend(format!("decode manifest_json: {e}")))?;
        let status = ProposalStatus::parse(&self.status).ok_or_else(|| {
            SkillAuthorError::Backend(format!("unknown proposal status {:?}", self.status))
        })?;
        Ok(ProposalRow {
            id: self.id,
            // tenant_id is vestigial under the single-user pivot (no column);
            // synthesise the owner tenant so the public domain shape is preserved.
            tenant_id: xiaoguai_storage::OWNER_TENANT_ID.to_string(),
            proposed_by: self.proposed_by,
            manifest,
            status,
            reason: self.reason,
            created_at: self.created_at,
            decided_at: self.decided_at,
            decided_by: self.decided_by,
        })
    }
}

#[allow(clippy::needless_pass_by_value)]
fn pg_err(e: sqlx::Error) -> SkillAuthorError {
    SkillAuthorError::Backend(e.to_string())
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = e {
        // SQLite extended result codes for UNIQUE/PK constraint violations.
        if matches!(db.code().as_deref(), Some("2067" | "1555")) {
            return true;
        }
        return db.message().contains("UNIQUE constraint failed");
    }
    false
}

// ---------------------------------------------------------------------------
// PgSkillProposalRepository
// ---------------------------------------------------------------------------

/// Postgres-backed `SkillProposalRepository` against `skill_proposals`.
#[derive(Debug, Clone)]
pub struct PgSkillProposalRepository {
    pool: SqlitePool,
}

impl PgSkillProposalRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<dyn SkillProposalRepository> {
        Arc::new(Self::new(pool))
    }
}

#[async_trait]
impl SkillProposalRepository for PgSkillProposalRepository {
    async fn insert(&self, row: ProposalRow) -> Result<ProposalRow, SkillAuthorError> {
        let manifest_json = serde_json::to_value(&row.manifest)
            .map_err(|e| SkillAuthorError::Backend(format!("encode manifest: {e}")))?;
        let status_str = row.status.as_str();
        let description = row.manifest.description.clone();
        let result = sqlx::query(
            "INSERT INTO skill_proposals \
                (id, proposed_by, name, description, version, \
                 manifest_json, status, reason, created_at, decided_at, decided_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.proposed_by)
        .bind(&row.manifest.name)
        .bind(&description)
        .bind(&row.manifest.version)
        .bind(sqlx::types::Json(&manifest_json))
        .bind(status_str)
        .bind(row.reason.as_deref())
        .bind(row.created_at)
        .bind(row.decided_at)
        .bind(row.decided_by.as_deref())
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(row),
            Err(e) if is_unique_violation(&e) => Err(SkillAuthorError::Duplicate),
            Err(e) => Err(pg_err(e)),
        }
    }

    async fn get(&self, id: &str) -> Result<Option<ProposalRow>, SkillAuthorError> {
        let row: Option<ProposalDbRow> = sqlx::query_as(
            "SELECT id, proposed_by, manifest_json, status, reason, \
                    created_at, decided_at, decided_by \
             FROM skill_proposals \
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?;
        row.map(ProposalDbRow::try_into_domain).transpose()
    }

    async fn list(
        &self,
        tenant_id: &str,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<ProposalRow>, SkillAuthorError> {
        let _ = tenant_id; // vestigial under single-user pivot
        let rows: Vec<ProposalDbRow> = if let Some(s) = status {
            sqlx::query_as(
                "SELECT id, proposed_by, manifest_json, status, reason, \
                        created_at, decided_at, decided_by \
                 FROM skill_proposals \
                 WHERE status = ? \
                 ORDER BY created_at DESC",
            )
            .bind(s.as_str())
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(
                "SELECT id, proposed_by, manifest_json, status, reason, \
                        created_at, decided_at, decided_by \
                 FROM skill_proposals \
                 ORDER BY created_at DESC",
            )
            .fetch_all(&self.pool)
            .await
        }
        .map_err(pg_err)?;

        rows.into_iter()
            .map(ProposalDbRow::try_into_domain)
            .collect()
    }

    async fn set_status(
        &self,
        id: &str,
        status: ProposalStatus,
        decided_by: &str,
        reason: Option<&str>,
    ) -> Result<ProposalRow, SkillAuthorError> {
        let now = Utc::now();
        let updated: Option<ProposalDbRow> = sqlx::query_as(
            "UPDATE skill_proposals \
             SET status = ?2, decided_by = ?3, decided_at = ?4, \
                 reason = COALESCE(?5, reason) \
             WHERE id = ?1 \
             RETURNING id, proposed_by, manifest_json, status, reason, \
                       created_at, decided_at, decided_by",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(decided_by)
        .bind(now)
        .bind(reason)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?;

        updated
            .ok_or(SkillAuthorError::NotFound)
            .and_then(ProposalDbRow::try_into_domain)
    }
}

// ---------------------------------------------------------------------------
// PgTenantSettings — reads `allow_skill_authoring` from JSONB
// ---------------------------------------------------------------------------

/// Postgres-backed `TenantSettingsReader` against `tenant_settings`.
#[derive(Debug, Clone)]
pub struct PgTenantSettings {
    pool: SqlitePool,
}

impl PgTenantSettings {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<dyn TenantSettingsReader> {
        Arc::new(Self::new(pool))
    }
}

#[async_trait]
impl TenantSettingsReader for PgTenantSettings {
    async fn allow_skill_authoring(&self, tenant_id: &str) -> Result<bool, SkillAuthorError> {
        // `settings->>'allow_skill_authoring'` returns the JSON value as
        // text, or NULL if either the row is missing or the key is absent.
        // `'true'` is the only enabling value (case-sensitive). Any other
        // text (including `'false'`, `'1'`, anything malformed) maps to
        // disabled — fail-closed.
        let _ = tenant_id; // vestigial under single-user pivot
        let val: Option<String> = sqlx::query_scalar(
            "SELECT json_extract(settings, '$.allow_skill_authoring') \
             FROM tenant_settings \
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?
        .flatten();
        Ok(val.as_deref() == Some("true"))
    }

    async fn sandbox_tier(
        &self,
        tenant_id: &str,
    ) -> Result<crate::skill_author::SandboxTier, SkillAuthorError> {
        // DEC-019: `settings->>'sandbox_tier'` is parsed lenient (case
        // insensitive, "L3" → L3, anything else → L1). Missing row /
        // missing key → L1 (safe default per PHILO §14).
        let _ = tenant_id; // vestigial under single-user pivot
        let val: Option<String> = sqlx::query_scalar(
            "SELECT json_extract(settings, '$.sandbox_tier') \
             FROM tenant_settings \
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?
        .flatten();
        Ok(val
            .as_deref()
            .map_or(crate::skill_author::SandboxTier::L1, |s| {
                crate::skill_author::SandboxTier::from_str_lenient(s)
            }))
    }
}

// ---------------------------------------------------------------------------
// Tests — embedded SQLite (single-user pivot, DEC-033)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_author::SkillManifest;
    use uuid::Uuid;

    /// Open a fresh migrated temp-file `SQLite` pool. Returns the pool plus the
    /// `TempDir` guard, which the caller must keep alive for the test's
    /// duration (dropping it deletes the database file).
    async fn sqlite_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let pool = xiaoguai_storage::db::connect(path.to_str().unwrap(), 5)
            .await
            .unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();
        (pool, dir)
    }

    fn manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.into(),
            description: "test skill for skill_author_pg".into(),
            version: "0.1.0".into(),
            system_prompt: "You are a test skill.".into(),
            tool_allowlist: vec!["echo".into()],
        }
    }

    fn row(tenant: &str, name: &str) -> ProposalRow {
        ProposalRow {
            id: Uuid::new_v4().to_string(),
            tenant_id: tenant.to_string(),
            proposed_by: "agent:test".into(),
            manifest: manifest(name),
            status: ProposalStatus::Pending,
            reason: None,
            created_at: Utc::now(),
            decided_at: None,
            decided_by: None,
        }
    }

    #[tokio::test]
    async fn skill_author_pg_insert_get_list_set_status() {
        let (pool, _dir) = sqlite_pool().await;
        let repo = PgSkillProposalRepository::new(pool);
        let tid = Uuid::new_v4().to_string();
        let inserted = repo.insert(row(&tid, "test-skill")).await.unwrap();
        let got = repo.get(&inserted.id).await.unwrap().expect("present");
        assert_eq!(got.id, inserted.id);
        assert_eq!(got.status, ProposalStatus::Pending);

        let listed = repo.list(&tid, None).await.unwrap();
        assert_eq!(listed.len(), 1);
        let pending = repo
            .list(&tid, Some(ProposalStatus::Pending))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);

        let approved = repo
            .set_status(&inserted.id, ProposalStatus::Approved, "admin:zw", None)
            .await
            .unwrap();
        assert_eq!(approved.status, ProposalStatus::Approved);
        assert_eq!(approved.decided_by.as_deref(), Some("admin:zw"));
    }

    #[tokio::test]
    async fn skill_author_pg_duplicate_returns_duplicate() {
        let (pool, _dir) = sqlite_pool().await;
        let repo = PgSkillProposalRepository::new(pool);
        let tid = Uuid::new_v4().to_string();
        let r1 = row(&tid, "dup-skill");
        repo.insert(r1.clone()).await.unwrap();
        let r2 = ProposalRow {
            id: Uuid::new_v4().to_string(),
            ..r1.clone()
        };
        let err = repo.insert(r2).await.unwrap_err();
        assert!(matches!(err, SkillAuthorError::Duplicate), "got: {err:?}");
    }

    #[tokio::test]
    async fn tenant_settings_reader_returns_false_when_unset() {
        let (pool, _dir) = sqlite_pool().await;
        let settings = PgTenantSettings::new(pool);
        let unknown = Uuid::new_v4().to_string();
        assert!(!settings.allow_skill_authoring(&unknown).await.unwrap());
    }

    #[tokio::test]
    async fn tenant_settings_reader_returns_true_after_upsert() {
        let (pool, _dir) = sqlite_pool().await;
        let settings = PgTenantSettings::new(pool.clone());
        let tid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tenant_settings (tenant_id, settings) \
             VALUES (?, ?) \
             ON CONFLICT (tenant_id) DO UPDATE SET settings = EXCLUDED.settings",
        )
        .bind(&tid)
        .bind(r#"{"allow_skill_authoring":"true"}"#)
        .execute(&pool)
        .await
        .unwrap();
        assert!(settings.allow_skill_authoring(&tid).await.unwrap());
    }
}
