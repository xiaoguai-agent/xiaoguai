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
//! The HotL gate + audit-sink adapters live in
//! `xiaoguai-core::skill_author_bridge` so we don't pull an
//! `xiaoguai-api`/`xiaoguai-audit` dependency cycle back into this crate.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;

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
    tenant_id: String,
    proposed_by: String,
    manifest_json: JsonValue,
    status: String,
    reason: Option<String>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
    decided_by: Option<String>,
}

impl ProposalDbRow {
    fn try_into_domain(self) -> Result<ProposalRow, SkillAuthorError> {
        let manifest: SkillManifest =
            serde_json::from_value(self.manifest_json).map_err(|e| {
                SkillAuthorError::Backend(format!("decode manifest_json: {e}"))
            })?;
        let status = ProposalStatus::parse(&self.status).ok_or_else(|| {
            SkillAuthorError::Backend(format!("unknown proposal status {:?}", self.status))
        })?;
        Ok(ProposalRow {
            id: self.id,
            tenant_id: self.tenant_id,
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
        return db.code().as_deref() == Some("23505");
    }
    false
}

// ---------------------------------------------------------------------------
// PgSkillProposalRepository
// ---------------------------------------------------------------------------

/// Postgres-backed `SkillProposalRepository` against `skill_proposals`.
#[derive(Debug, Clone)]
pub struct PgSkillProposalRepository {
    pool: PgPool,
}

impl PgSkillProposalRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<dyn SkillProposalRepository> {
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
                (id, tenant_id, proposed_by, name, description, version, \
                 manifest_json, status, reason, created_at, decided_at, decided_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(&row.id)
        .bind(&row.tenant_id)
        .bind(&row.proposed_by)
        .bind(&row.manifest.name)
        .bind(&description)
        .bind(&row.manifest.version)
        .bind(&manifest_json)
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
            "SELECT id, tenant_id, proposed_by, manifest_json, status, reason, \
                    created_at, decided_at, decided_by \
             FROM skill_proposals \
             WHERE id = $1",
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
        let rows: Vec<ProposalDbRow> = if let Some(s) = status {
            sqlx::query_as(
                "SELECT id, tenant_id, proposed_by, manifest_json, status, reason, \
                        created_at, decided_at, decided_by \
                 FROM skill_proposals \
                 WHERE tenant_id = $1 AND status = $2 \
                 ORDER BY created_at DESC",
            )
            .bind(tenant_id)
            .bind(s.as_str())
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(
                "SELECT id, tenant_id, proposed_by, manifest_json, status, reason, \
                        created_at, decided_at, decided_by \
                 FROM skill_proposals \
                 WHERE tenant_id = $1 \
                 ORDER BY created_at DESC",
            )
            .bind(tenant_id)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(pg_err)?;

        rows.into_iter().map(ProposalDbRow::try_into_domain).collect()
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
             SET status = $2, decided_by = $3, decided_at = $4, \
                 reason = COALESCE($5, reason) \
             WHERE id = $1 \
             RETURNING id, tenant_id, proposed_by, manifest_json, status, reason, \
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
    pool: PgPool,
}

impl PgTenantSettings {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<dyn TenantSettingsReader> {
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
        let val: Option<String> = sqlx::query_scalar(
            "SELECT settings->>'allow_skill_authoring' \
             FROM tenant_settings \
             WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_err)?
        .flatten();
        Ok(val.as_deref() == Some("true"))
    }
}

// ---------------------------------------------------------------------------
// Tests — gated on a live PG instance
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_author::SkillManifest;
    use uuid::Uuid;

    async fn pg_pool() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for skill_author_pg tests");
        PgPool::connect(&url).await.expect("pg connect")
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
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skill_author_pg_insert_get_list_set_status() {
        let pool = pg_pool().await;
        let repo = PgSkillProposalRepository::new(pool);
        let tid = Uuid::new_v4().to_string();
        let inserted = repo.insert(row(&tid, "test-skill")).await.unwrap();
        let got = repo.get(&inserted.id).await.unwrap().expect("present");
        assert_eq!(got.id, inserted.id);
        assert_eq!(got.status, ProposalStatus::Pending);

        let listed = repo.list(&tid, None).await.unwrap();
        assert_eq!(listed.len(), 1);
        let pending = repo.list(&tid, Some(ProposalStatus::Pending)).await.unwrap();
        assert_eq!(pending.len(), 1);

        let approved = repo
            .set_status(&inserted.id, ProposalStatus::Approved, "admin:zw", None)
            .await
            .unwrap();
        assert_eq!(approved.status, ProposalStatus::Approved);
        assert_eq!(approved.decided_by.as_deref(), Some("admin:zw"));
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn skill_author_pg_duplicate_returns_duplicate() {
        let pool = pg_pool().await;
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
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn tenant_settings_reader_returns_false_when_unset() {
        let pool = pg_pool().await;
        let settings = PgTenantSettings::new(pool);
        let unknown = Uuid::new_v4().to_string();
        assert!(!settings.allow_skill_authoring(&unknown).await.unwrap());
    }

    #[tokio::test]
    #[ignore = "requires live PG; run with DATABASE_URL set"]
    async fn tenant_settings_reader_returns_true_after_upsert() {
        let pool = pg_pool().await;
        let settings = PgTenantSettings::new(pool.clone());
        let tid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tenant_settings (tenant_id, settings) \
             VALUES ($1, $2::jsonb) \
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
