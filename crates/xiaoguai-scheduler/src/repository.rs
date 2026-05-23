//! Repository traits + in-memory impls.
//!
//! In keeping with the rest of the workspace (see `xiaoguai-storage`
//! repositories + `xiaoguai-rag` `InMemoryRagClient`), the production
//! contract is the trait; in-memory impls back tests and let the
//! runner be exercised without booting Postgres. PG-backed impls are
//! added together with the `xiaoguai-runtime` extraction in v0.12.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use thiserror::Error;

use crate::job::{JobRun, JobRunStatus, ScheduledJob};

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("backend: {0}")]
    Backend(String),
}

pub type RepoResult<T> = Result<T, RepoError>;

#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn upsert(&self, job: &ScheduledJob) -> RepoResult<()>;
    async fn get(&self, id: &str) -> RepoResult<ScheduledJob>;
    async fn list_due(&self, now: DateTime<Utc>, limit: usize) -> RepoResult<Vec<ScheduledJob>>;
    /// Update bookkeeping fields after the runner advances a job: the
    /// computed next fire time + the moment it fired.
    async fn record_fire(
        &self,
        id: &str,
        last_fire_at: DateTime<Utc>,
        next_fire_at: Option<DateTime<Utc>>,
    ) -> RepoResult<()>;
}

#[async_trait]
pub trait JobRunRepository: Send + Sync {
    /// Insert a new run row and return it with its assigned `id`.
    async fn insert(&self, run: JobRun) -> RepoResult<JobRun>;
    async fn update_status(
        &self,
        id: i64,
        status: JobRunStatus,
        finished_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
        output_preview: Option<String>,
        session_id: Option<String>,
    ) -> RepoResult<()>;
    async fn list_for_job(&self, job_id: &str, limit: usize) -> RepoResult<Vec<JobRun>>;
}

#[derive(Default)]
pub struct InMemoryJobRepository {
    jobs: Mutex<HashMap<String, ScheduledJob>>,
}

impl InMemoryJobRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl JobRepository for InMemoryJobRepository {
    async fn upsert(&self, job: &ScheduledJob) -> RepoResult<()> {
        let mut g = self.jobs.lock();
        g.insert(job.id.clone(), job.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> RepoResult<ScheduledJob> {
        let g = self.jobs.lock();
        g.get(id)
            .cloned()
            .ok_or_else(|| RepoError::NotFound(id.into()))
    }

    async fn list_due(&self, now: DateTime<Utc>, limit: usize) -> RepoResult<Vec<ScheduledJob>> {
        let g = self.jobs.lock();
        let mut out: Vec<ScheduledJob> = g
            .values()
            .filter(|j| j.enabled)
            // Reactive triggers (file_watch / webhook / git_push /
            // db_poll) fire via the TriggerEvent channel, never via
            // this scheduled scan — skip them unconditionally.
            .filter(|j| j.trigger.is_scheduled())
            .filter(|j| match j.next_fire_at {
                Some(t) => t <= now,
                // No next_fire_at yet — treat as "fire immediately" so
                // the runner can prime the schedule on its first tick.
                None => true,
            })
            .cloned()
            .collect();
        out.sort_by_key(|j| j.next_fire_at.unwrap_or(j.created_at));
        out.truncate(limit);
        Ok(out)
    }

    async fn record_fire(
        &self,
        id: &str,
        last_fire_at: DateTime<Utc>,
        next_fire_at: Option<DateTime<Utc>>,
    ) -> RepoResult<()> {
        let mut g = self.jobs.lock();
        let j = g
            .get_mut(id)
            .ok_or_else(|| RepoError::NotFound(id.into()))?;
        j.last_fire_at = Some(last_fire_at);
        j.next_fire_at = next_fire_at;
        j.updated_at = Utc::now();
        Ok(())
    }
}

pub struct InMemoryJobRunRepository {
    next_id: AtomicI64,
    runs: Mutex<Vec<JobRun>>,
}

impl Default for InMemoryJobRunRepository {
    fn default() -> Self {
        Self {
            next_id: AtomicI64::new(1),
            runs: Mutex::new(Vec::new()),
        }
    }
}

impl InMemoryJobRunRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only: snapshot all rows currently stored.
    #[must_use]
    pub fn snapshot(&self) -> Vec<JobRun> {
        self.runs.lock().clone()
    }
}

#[async_trait]
impl JobRunRepository for InMemoryJobRunRepository {
    async fn insert(&self, mut run: JobRun) -> RepoResult<JobRun> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        run.id = id;
        self.runs.lock().push(run.clone());
        Ok(run)
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
        let mut g = self.runs.lock();
        let r = g
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| RepoError::NotFound(format!("run:{id}")))?;
        r.status = status;
        if finished_at.is_some() {
            r.finished_at = finished_at;
        }
        if error_message.is_some() {
            r.error_message = error_message;
        }
        if output_preview.is_some() {
            r.output_preview = output_preview;
        }
        if session_id.is_some() {
            r.session_id = session_id;
        }
        Ok(())
    }

    async fn list_for_job(&self, job_id: &str, limit: usize) -> RepoResult<Vec<JobRun>> {
        let g = self.runs.lock();
        let mut out: Vec<JobRun> = g.iter().filter(|r| r.job_id == job_id).cloned().collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.id));
        out.truncate(limit);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{JobRun, JobRunStatus, ScheduledJob};
    use crate::trigger::Trigger;

    fn sample_job(id: &str) -> ScheduledJob {
        ScheduledJob::new(
            id,
            Some("tenant-x".into()),
            id,
            Trigger::interval(60).unwrap(),
            serde_json::json!({"prompt": "hello"}),
        )
    }

    #[tokio::test]
    async fn list_due_returns_jobs_with_no_next_fire() {
        let repo = InMemoryJobRepository::new();
        let job = sample_job("j1");
        repo.upsert(&job).await.unwrap();
        let due = repo.list_due(Utc::now(), 10).await.unwrap();
        assert_eq!(due.len(), 1);
    }

    #[tokio::test]
    async fn record_fire_updates_bookkeeping() {
        let repo = InMemoryJobRepository::new();
        let job = sample_job("j1");
        repo.upsert(&job).await.unwrap();
        let now = Utc::now();
        let next = now + chrono::Duration::seconds(60);
        repo.record_fire("j1", now, Some(next)).await.unwrap();
        let back = repo.get("j1").await.unwrap();
        assert_eq!(back.last_fire_at, Some(now));
        assert_eq!(back.next_fire_at, Some(next));
    }

    #[tokio::test]
    async fn run_repo_assigns_monotonic_ids() {
        let repo = InMemoryJobRunRepository::new();
        let mk = |attempt: u32| JobRun {
            id: 0,
            job_id: "j1".into(),
            tenant_id: Some("t1".into()),
            status: JobRunStatus::Pending,
            attempt,
            started_at: None,
            finished_at: None,
            session_id: None,
            error_message: None,
            output_preview: None,
            created_at: Utc::now(),
        };
        let a = repo.insert(mk(1)).await.unwrap();
        let b = repo.insert(mk(2)).await.unwrap();
        assert!(a.id < b.id);
    }
}
