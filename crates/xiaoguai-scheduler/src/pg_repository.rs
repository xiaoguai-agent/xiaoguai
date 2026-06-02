//! SQLite-backed [`JobRepository`] + [`JobRunRepository`] (DEC-033 single-user).
//!
//! The in-memory impls in [`crate::repository`] cover tests and the
//! `RunnerOptions::default` operator path. These persist `scheduled_jobs` /
//! `scheduled_job_runs` durably across restarts.
//!
//! Schema: migration `0007_scheduled_jobs.sql`. `tenant_id` was dropped under
//! the single-user pivot, so the (vestigial) `tenant_id` on the domain types is
//! neither stored nor read back (it resolves to `None`). Each write opens a
//! plain transaction via [`xiaoguai_storage::repositories::begin_tenant_tx`]
//! (the `tenant` argument is ignored — there is no RLS under SQLite).

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use xiaoguai_storage::repositories::begin_tenant_tx;

use crate::job::{JobRun, JobRunStatus, ScheduledJob};
use crate::repository::{JobRepository, JobRunRepository, RepoError, RepoResult};
use crate::retry::RetryPolicy;
use crate::trigger::Trigger;

pub struct PgJobRepository {
    pool: SqlitePool,
}

impl PgJobRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobRepository for PgJobRepository {
    async fn upsert(&self, job: &ScheduledJob) -> RepoResult<()> {
        let mut tx = begin_tenant_tx(&self.pool, job.tenant_id.as_deref())
            .await
            .map_err(repo_err)?;
        sqlx::query(
            "INSERT INTO scheduled_jobs
                (id, name, description, trigger, payload, retry_policy, sinks,
                 enabled, next_fire_at, last_fire_at, created_at, updated_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT (id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                trigger = excluded.trigger,
                payload = excluded.payload,
                retry_policy = excluded.retry_policy,
                sinks = excluded.sinks,
                enabled = excluded.enabled,
                next_fire_at = excluded.next_fire_at,
                last_fire_at = excluded.last_fire_at,
                updated_at = excluded.updated_at",
        )
        .bind(&job.id)
        .bind(&job.name)
        .bind(job.description.as_deref())
        .bind(serde_json::to_value(&job.trigger).map_err(serde_err)?)
        .bind(&job.payload)
        .bind(serde_json::to_value(&job.retry_policy).map_err(serde_err)?)
        .bind(serde_json::to_value(&job.sinks).map_err(serde_err)?)
        .bind(job.enabled)
        .bind(job.next_fire_at)
        .bind(job.last_fire_at)
        .bind(job.created_at)
        .bind(job.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(sqlx_err)?;
        tx.commit().await.map_err(sqlx_err)?;
        Ok(())
    }

    async fn get(&self, id: &str) -> RepoResult<ScheduledJob> {
        let row = sqlx::query("SELECT * FROM scheduled_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(sqlx_err)?
            .ok_or_else(|| RepoError::NotFound(id.into()))?;
        row_to_job(&row)
    }

    async fn list_due(&self, now: DateTime<Utc>, limit: usize) -> RepoResult<Vec<ScheduledJob>> {
        let rows = sqlx::query(
            "SELECT * FROM scheduled_jobs
             WHERE enabled IS TRUE
               AND (next_fire_at IS NULL OR next_fire_at <= ?1)
             ORDER BY COALESCE(next_fire_at, created_at)
             LIMIT ?2",
        )
        .bind(now)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_err)?;

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let job = row_to_job(r)?;
            if job.trigger.is_scheduled() {
                out.push(job);
            }
        }
        Ok(out)
    }

    async fn record_fire(
        &self,
        id: &str,
        last_fire_at: DateTime<Utc>,
        next_fire_at: Option<DateTime<Utc>>,
    ) -> RepoResult<()> {
        let updated = sqlx::query(
            "UPDATE scheduled_jobs
             SET last_fire_at = ?2, next_fire_at = ?3, updated_at = datetime('now')
             WHERE id = ?1",
        )
        .bind(id)
        .bind(last_fire_at)
        .bind(next_fire_at)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        if updated.rows_affected() == 0 {
            return Err(RepoError::NotFound(id.into()));
        }
        Ok(())
    }

    async fn list_reactive(&self) -> RepoResult<Vec<ScheduledJob>> {
        // Push the type filter into SQL so we don't pull every enabled job
        // through the application layer. `json_extract(trigger, '$.type')` reads
        // the discriminant out of the JSON-TEXT column populated by serde's
        // `#[serde(tag = "type")]` representation.
        let rows = sqlx::query(
            "SELECT * FROM scheduled_jobs
             WHERE enabled IS TRUE
               AND json_extract(trigger, '$.type') IN ('file_watch','webhook','git_push','db_poll')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_err)?;

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let job = row_to_job(r)?;
            if job.trigger.is_reactive() {
                out.push(job);
            }
        }
        Ok(out)
    }
}

pub struct PgJobRunRepository {
    pool: SqlitePool,
}

impl PgJobRunRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobRunRepository for PgJobRunRepository {
    async fn insert(&self, run: JobRun) -> RepoResult<JobRun> {
        let mut tx = begin_tenant_tx(&self.pool, run.tenant_id.as_deref())
            .await
            .map_err(repo_err)?;
        let row = sqlx::query(
            "INSERT INTO scheduled_job_runs
                (job_id, status, attempt, started_at, finished_at,
                 session_id, error_message, output_preview, created_at)
             VALUES (?,?,?,?,?,?,?,?,?)
             RETURNING id",
        )
        .bind(&run.job_id)
        .bind(run.status.as_str())
        .bind(i32::try_from(run.attempt).unwrap_or(i32::MAX))
        .bind(run.started_at)
        .bind(run.finished_at)
        .bind(run.session_id.as_deref())
        .bind(run.error_message.as_deref())
        .bind(run.output_preview.as_deref())
        .bind(run.created_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(sqlx_err)?;
        tx.commit().await.map_err(sqlx_err)?;
        let id: i64 = row.try_get("id").map_err(sqlx_err)?;
        Ok(JobRun { id, ..run })
    }

    async fn update_status(
        &self,
        id: i64,
        status: JobRunStatus,
        finished_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
        output_preview: Option<String>,
        session_id: Option<String>,
    ) -> RepoResult<()> {
        let updated = sqlx::query(
            "UPDATE scheduled_job_runs
             SET status = ?2,
                 finished_at = COALESCE(?3, finished_at),
                 error_message = COALESCE(?4, error_message),
                 output_preview = COALESCE(?5, output_preview),
                 session_id = COALESCE(?6, session_id)
             WHERE id = ?1",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(finished_at)
        .bind(error_message)
        .bind(output_preview)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        if updated.rows_affected() == 0 {
            return Err(RepoError::NotFound(format!("run:{id}")));
        }
        Ok(())
    }

    async fn list_for_job(&self, job_id: &str, limit: usize) -> RepoResult<Vec<JobRun>> {
        let rows = sqlx::query(
            "SELECT * FROM scheduled_job_runs
             WHERE job_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )
        .bind(job_id)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_err)?;
        rows.iter().map(row_to_run).collect()
    }
}

fn row_to_job(r: &sqlx::sqlite::SqliteRow) -> RepoResult<ScheduledJob> {
    let trigger: serde_json::Value = r.try_get("trigger").map_err(sqlx_err)?;
    let trigger: Trigger = serde_json::from_value(trigger).map_err(serde_err)?;
    let payload: serde_json::Value = r.try_get("payload").map_err(sqlx_err)?;
    let retry_policy: serde_json::Value = r.try_get("retry_policy").map_err(sqlx_err)?;
    let retry_policy: RetryPolicy = serde_json::from_value(retry_policy).map_err(serde_err)?;
    let sinks: serde_json::Value = r.try_get("sinks").map_err(sqlx_err)?;
    let sinks: Vec<String> = serde_json::from_value(sinks).map_err(serde_err)?;
    Ok(ScheduledJob {
        id: r.try_get("id").map_err(sqlx_err)?,
        // tenant_id column dropped under the single-user pivot.
        tenant_id: None,
        name: r.try_get("name").map_err(sqlx_err)?,
        description: r.try_get("description").map_err(sqlx_err)?,
        trigger,
        payload,
        retry_policy,
        sinks,
        enabled: r.try_get("enabled").map_err(sqlx_err)?,
        next_fire_at: r.try_get("next_fire_at").map_err(sqlx_err)?,
        last_fire_at: r.try_get("last_fire_at").map_err(sqlx_err)?,
        created_at: r.try_get("created_at").map_err(sqlx_err)?,
        updated_at: r.try_get("updated_at").map_err(sqlx_err)?,
    })
}

fn row_to_run(r: &sqlx::sqlite::SqliteRow) -> RepoResult<JobRun> {
    let status_str: String = r.try_get("status").map_err(sqlx_err)?;
    let status = parse_status(&status_str)?;
    let attempt: i32 = r.try_get("attempt").map_err(sqlx_err)?;
    Ok(JobRun {
        id: r.try_get("id").map_err(sqlx_err)?,
        job_id: r.try_get("job_id").map_err(sqlx_err)?,
        // tenant_id column dropped under the single-user pivot.
        tenant_id: None,
        status,
        attempt: u32::try_from(attempt).unwrap_or(0),
        started_at: r.try_get("started_at").map_err(sqlx_err)?,
        finished_at: r.try_get("finished_at").map_err(sqlx_err)?,
        session_id: r.try_get("session_id").map_err(sqlx_err)?,
        error_message: r.try_get("error_message").map_err(sqlx_err)?,
        output_preview: r.try_get("output_preview").map_err(sqlx_err)?,
        created_at: r.try_get("created_at").map_err(sqlx_err)?,
    })
}

fn parse_status(s: &str) -> RepoResult<JobRunStatus> {
    match s {
        "pending" => Ok(JobRunStatus::Pending),
        "running" => Ok(JobRunStatus::Running),
        "succeeded" => Ok(JobRunStatus::Succeeded),
        "failed" => Ok(JobRunStatus::Failed),
        "cancelled" => Ok(JobRunStatus::Cancelled),
        other => Err(RepoError::Backend(format!("unknown status {other}"))),
    }
}

// These three closures are passed by value to `Result::map_err`, which
// owns the error — taking `&E` would force `.map_err(|e| sqlx_err(&e))`
// at every call site. clippy's needless_pass_by_value lint doesn't fit
// this shape.
#[allow(clippy::needless_pass_by_value)]
fn sqlx_err(e: sqlx::Error) -> RepoError {
    RepoError::Backend(e.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn serde_err(e: serde_json::Error) -> RepoError {
    RepoError::Backend(format!("serde: {e}"))
}

#[allow(clippy::needless_pass_by_value)]
fn repo_err(e: xiaoguai_storage::repositories::error::RepoError) -> RepoError {
    RepoError::Backend(e.to_string())
}

/// Quick wait so tests can be reasonably deterministic about commit
/// ordering when assertions follow an insert. The repo itself doesn't
/// sleep; the helper is only used by callers (e.g. tests).
#[allow(dead_code)]
pub(crate) const POST_INSERT_SETTLE: Duration = Duration::from_millis(10);
