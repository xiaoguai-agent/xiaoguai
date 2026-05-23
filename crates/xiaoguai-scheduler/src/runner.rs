//! The runner — picks due jobs, fires them, records `JobRun` rows,
//! writes `audit_log` entries, and pushes results to sinks.
//!
//! The runner is intentionally *synchronous in its decisions*: one
//! call to [`JobRunner::tick`] walks the repo, fires every due job
//! once, and returns. The orchestration loop (sleep ⇄ tick) is left
//! to the binary so tests can drive it deterministically.
//!
//! Retry semantics:
//!
//! * Attempt 1 fires immediately when the job is due.
//! * On failure, the runner consults [`RetryPolicy::delay_before_attempt`]
//!   to compute the backoff for attempt N+1.
//! * If that returns `None`, the `JobRun` is marked `Failed` and the
//!   scheduler moves on (no further retries until the next trigger fire).
//! * If it returns `Some(d)`, the runner sleeps `d` and tries again.
//!
//! The whole retry sequence for a single fire produces one `JobRun`
//! row *per attempt* — linear audit trail.

use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;
use xiaoguai_audit::AuditEntry;

use crate::audit::AuditAppender;
use crate::executor::{ExecutionOutcome, JobExecutor};
use crate::job::{JobRun, JobRunStatus, ScheduledJob};
use crate::repository::{JobRepository, JobRunRepository, RepoError};
use crate::sink::{PushPayload, PushSink};

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("repository: {0}")]
    Repository(#[from] RepoError),
    #[error("audit: {0}")]
    Audit(String),
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    /// Max jobs to fire in one tick. Acts as a fairness bound — a
    /// single misconfigured job can't starve the scheduler.
    pub max_jobs_per_tick: usize,
    /// Hard cap on total time the runner will sleep waiting for
    /// retries inside a single tick. Defaults to zero — production
    /// wiring sets a small non-zero value (≤30s); the test harness
    /// can leave it zero and consume backoffs by stubbing the clock.
    pub max_retry_sleep_secs: u64,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            max_jobs_per_tick: 32,
            max_retry_sleep_secs: 0,
        }
    }
}

pub struct JobRunner {
    jobs: Arc<dyn JobRepository>,
    runs: Arc<dyn JobRunRepository>,
    executor: Arc<dyn JobExecutor>,
    audit: Arc<dyn AuditAppender>,
    sinks: Vec<Arc<dyn PushSink>>,
    options: RunnerOptions,
}

impl JobRunner {
    pub fn new(
        jobs: Arc<dyn JobRepository>,
        runs: Arc<dyn JobRunRepository>,
        executor: Arc<dyn JobExecutor>,
        audit: Arc<dyn AuditAppender>,
    ) -> Self {
        Self {
            jobs,
            runs,
            executor,
            audit,
            sinks: Vec::new(),
            options: RunnerOptions::default(),
        }
    }

    #[must_use]
    pub fn with_options(mut self, options: RunnerOptions) -> Self {
        self.options = options;
        self
    }

    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn PushSink>) -> Self {
        self.sinks.push(sink);
        self
    }

    /// Pick up to `max_jobs_per_tick` due jobs and run each (with
    /// retries) to completion. Returns the number of jobs fired
    /// (one per due job, regardless of retry count).
    pub async fn tick(&self) -> Result<usize, RunnerError> {
        let now = Utc::now();
        let due = self
            .jobs
            .list_due(now, self.options.max_jobs_per_tick)
            .await?;
        let fired = due.len();
        for job in due {
            self.fire(job).await?;
        }
        Ok(fired)
    }

    /// Fire a single job once (with its retry sequence) and advance
    /// the schedule for the next fire.
    pub async fn fire(&self, job: ScheduledJob) -> Result<(), RunnerError> {
        let fired_at = Utc::now();
        let max_attempts = job.retry_policy.max_attempts.max(1);

        for attempt in 1..=max_attempts {
            // Sleep before the attempt if the retry policy says so.
            // delay_before_attempt(1) is always zero, so attempt 1
            // doesn't sleep.
            if let Some(delay) = job.retry_policy.delay_before_attempt(attempt) {
                if !delay.is_zero() && delay.as_secs() <= self.options.max_retry_sleep_secs {
                    tokio::time::sleep(delay).await;
                }
            }

            let run = JobRun {
                id: 0,
                job_id: job.id.clone(),
                tenant_id: job.tenant_id.clone(),
                status: JobRunStatus::Running,
                attempt,
                started_at: Some(Utc::now()),
                finished_at: None,
                session_id: None,
                error_message: None,
                output_preview: None,
                created_at: Utc::now(),
            };
            let inserted = self.runs.insert(run).await?;

            let result = self.executor.execute(&job, attempt).await;

            let final_status;
            let outcome_for_push: ExecutionOutcome;
            let error_message: Option<String>;
            match result {
                Ok(outcome) => {
                    self.runs
                        .update_status(
                            inserted.id,
                            JobRunStatus::Succeeded,
                            Some(Utc::now()),
                            None,
                            Some(outcome.output_preview.clone()),
                            outcome.session_id.clone(),
                        )
                        .await?;
                    final_status = JobRunStatus::Succeeded;
                    outcome_for_push = outcome;
                    error_message = None;
                }
                Err(err) => {
                    let last = attempt == max_attempts;
                    let status = if last {
                        JobRunStatus::Failed
                    } else {
                        // Mid-sequence retryable failure — record as
                        // Failed for this attempt; the next loop iter
                        // inserts a fresh row.
                        JobRunStatus::Failed
                    };
                    self.runs
                        .update_status(
                            inserted.id,
                            status,
                            Some(Utc::now()),
                            Some(err.clone()),
                            None,
                            None,
                        )
                        .await?;
                    final_status = status;
                    outcome_for_push = ExecutionOutcome {
                        output_preview: String::new(),
                        session_id: None,
                    };
                    error_message = Some(err);
                }
            }

            // Audit every attempt — the chain is the source of truth
            // for the audit-first console.
            self.write_audit(&job, &inserted, final_status, error_message.as_deref())
                .await?;

            // Push to sinks on a successful run, or on the *final*
            // failed attempt. We deliberately do not push mid-retry
            // intermediate failures — sinks would get spammed.
            if matches!(final_status, JobRunStatus::Succeeded)
                || (matches!(final_status, JobRunStatus::Failed) && attempt == max_attempts)
            {
                self.push_to_sinks(
                    &job,
                    &inserted,
                    final_status,
                    &outcome_for_push,
                    error_message.as_deref(),
                )
                .await;
            }

            if matches!(final_status, JobRunStatus::Succeeded) {
                break;
            }
            // Failed; continue the retry loop unless we just exhausted attempts.
        }

        // Advance the schedule.
        let next = job
            .trigger
            .next_fire_after(fired_at)
            .map_err(|e| RunnerError::Audit(e.to_string()))?;
        self.jobs.record_fire(&job.id, fired_at, next).await?;
        Ok(())
    }

    async fn write_audit(
        &self,
        job: &ScheduledJob,
        run: &JobRun,
        status: JobRunStatus,
        error_message: Option<&str>,
    ) -> Result<(), RunnerError> {
        let mut details = serde_json::json!({
            "run_id": run.id,
            "attempt": run.attempt,
            "status": status.as_str(),
        });
        if let Some(err) = error_message {
            details["error"] = serde_json::Value::String(err.to_string());
        }
        if let Some(sid) = &run.session_id {
            details["session_id"] = serde_json::Value::String(sid.clone());
        }
        let entry = AuditEntry {
            ts: Utc::now(),
            tenant_id: job.tenant_id.clone().unwrap_or_else(|| "system".into()),
            actor: format!("scheduler:{}", job.id),
            action: "scheduler.job_run".into(),
            resource: Some(format!("job:{}", job.id)),
            details,
        };
        self.audit.append(entry).await.map_err(RunnerError::Audit)?;
        Ok(())
    }

    async fn push_to_sinks(
        &self,
        job: &ScheduledJob,
        run: &JobRun,
        status: JobRunStatus,
        outcome: &ExecutionOutcome,
        error_message: Option<&str>,
    ) {
        if self.sinks.is_empty() || job.sinks.is_empty() {
            return;
        }
        let payload = PushPayload {
            job_id: job.id.clone(),
            run_id: run.id,
            tenant_id: job.tenant_id.clone(),
            status: status.as_str().to_string(),
            fired_at: run.created_at,
            output_preview: if outcome.output_preview.is_empty() {
                None
            } else {
                Some(outcome.output_preview.clone())
            },
            error_message: error_message.map(str::to_string),
        };
        for sink in &self.sinks {
            if !job.sinks.iter().any(|s| s == sink.id()) {
                continue;
            }
            if let Err(e) = sink.deliver(&payload).await {
                tracing::warn!(sink = sink.id(), error = %e, "push sink delivery failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::RecordingAuditAppender;
    use crate::executor::EchoExecutor;
    use crate::job::ScheduledJob;
    use crate::repository::{InMemoryJobRepository, InMemoryJobRunRepository};
    use crate::retry::RetryPolicy;
    use crate::sink::LoggingSink;
    use crate::trigger::Trigger;
    use async_trait::async_trait;

    fn job(id: &str) -> ScheduledJob {
        ScheduledJob::new(
            id,
            Some("tenant-x".into()),
            id,
            Trigger::interval(60).unwrap(),
            serde_json::json!({"prompt": "hello"}),
        )
    }

    fn runner_with(
        executor: Arc<dyn JobExecutor>,
    ) -> (
        JobRunner,
        Arc<InMemoryJobRepository>,
        Arc<InMemoryJobRunRepository>,
        Arc<RecordingAuditAppender>,
    ) {
        let jobs: Arc<InMemoryJobRepository> = Arc::new(InMemoryJobRepository::new());
        let runs: Arc<InMemoryJobRunRepository> = Arc::new(InMemoryJobRunRepository::new());
        let audit: Arc<RecordingAuditAppender> = Arc::new(RecordingAuditAppender::new());
        let runner = JobRunner::new(jobs.clone(), runs.clone(), executor, audit.clone());
        (runner, jobs, runs, audit)
    }

    #[tokio::test]
    async fn successful_run_writes_one_audit_row() {
        let (runner, jobs, runs, audit) = runner_with(Arc::new(EchoExecutor));
        jobs.upsert(&job("j1")).await.unwrap();

        let fired = runner.tick().await.unwrap();
        assert_eq!(fired, 1);

        let runs_snap = runs.snapshot();
        assert_eq!(runs_snap.len(), 1);
        assert_eq!(runs_snap[0].status, JobRunStatus::Succeeded);
        assert_eq!(runs_snap[0].attempt, 1);
        assert!(runs_snap[0]
            .output_preview
            .as_deref()
            .unwrap()
            .contains("hello"));

        let entries = audit.snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "scheduler.job_run");
        assert_eq!(entries[0].actor, "scheduler:j1");
        assert_eq!(entries[0].tenant_id, "tenant-x");
        assert_eq!(entries[0].resource.as_deref(), Some("job:j1"));
        assert_eq!(
            entries[0].details["run_id"],
            serde_json::json!(runs_snap[0].id)
        );
        assert_eq!(entries[0].details["status"], serde_json::json!("succeeded"));
    }

    #[tokio::test]
    async fn next_fire_is_advanced_after_run() {
        let (runner, jobs, _runs, _audit) = runner_with(Arc::new(EchoExecutor));
        jobs.upsert(&job("j1")).await.unwrap();
        runner.tick().await.unwrap();
        let j = jobs.get("j1").await.unwrap();
        assert!(j.last_fire_at.is_some());
        assert!(j.next_fire_at.is_some());
        assert!(j.next_fire_at.unwrap() > j.last_fire_at.unwrap());
    }

    // Failing executor that fails the first N attempts then succeeds.
    struct FlakyExecutor {
        fail_until: u32,
    }
    #[async_trait]
    impl JobExecutor for FlakyExecutor {
        async fn execute(
            &self,
            _job: &ScheduledJob,
            attempt: u32,
        ) -> Result<ExecutionOutcome, String> {
            if attempt <= self.fail_until {
                Err(format!("flaky boom attempt={attempt}"))
            } else {
                Ok(ExecutionOutcome {
                    output_preview: format!("ok at attempt {attempt}"),
                    session_id: None,
                })
            }
        }
    }

    #[tokio::test]
    async fn retry_escalates_until_success() {
        // Fail twice, succeed on attempt 3. RetryPolicy default has
        // max_attempts = 3; we set zero backoff so the test is fast.
        let (mut runner, jobs, runs, audit) =
            runner_with(Arc::new(FlakyExecutor { fail_until: 2 }));
        // runner has zero max_retry_sleep_secs by default → sleeps skipped.
        let mut j = job("j1");
        j.retry_policy = RetryPolicy {
            max_attempts: 3,
            initial_backoff_secs: 0,
            multiplier: 1.0,
            max_backoff_secs: 0,
        };
        jobs.upsert(&j).await.unwrap();

        runner = runner.with_options(RunnerOptions::default());
        runner.tick().await.unwrap();

        let runs_snap = runs.snapshot();
        assert_eq!(runs_snap.len(), 3, "one row per attempt");
        assert_eq!(runs_snap[0].status, JobRunStatus::Failed);
        assert_eq!(runs_snap[1].status, JobRunStatus::Failed);
        assert_eq!(runs_snap[2].status, JobRunStatus::Succeeded);

        let entries = audit.snapshot();
        assert_eq!(entries.len(), 3, "one audit row per attempt");
        assert_eq!(entries[0].details["status"], serde_json::json!("failed"));
        assert_eq!(entries[2].details["status"], serde_json::json!("succeeded"));
    }

    #[tokio::test]
    async fn retry_exhausted_marks_failed_and_pushes_once() {
        let sink: Arc<LoggingSink> = Arc::new(LoggingSink::new("inbox"));
        let (mut runner, jobs, runs, audit) =
            runner_with(Arc::new(FlakyExecutor { fail_until: 99 }));
        runner = runner.with_sink(sink.clone());

        let mut j = job("j1");
        j.sinks = vec!["inbox".into()];
        j.retry_policy = RetryPolicy {
            max_attempts: 3,
            initial_backoff_secs: 0,
            multiplier: 1.0,
            max_backoff_secs: 0,
        };
        jobs.upsert(&j).await.unwrap();

        runner.tick().await.unwrap();

        let runs_snap = runs.snapshot();
        assert_eq!(runs_snap.len(), 3);
        assert!(runs_snap.iter().all(|r| r.status == JobRunStatus::Failed));

        // One audit row per attempt.
        assert_eq!(audit.snapshot().len(), 3);

        // Sink got exactly one delivery — on the final attempt.
        let captured = sink.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].status, "failed");
        assert!(captured[0].error_message.is_some());
    }

    #[tokio::test]
    async fn sink_only_delivers_when_job_lists_it() {
        let listed: Arc<LoggingSink> = Arc::new(LoggingSink::new("listed"));
        let unlisted: Arc<LoggingSink> = Arc::new(LoggingSink::new("unlisted"));
        let (mut runner, jobs, _runs, _audit) = runner_with(Arc::new(EchoExecutor));
        runner = runner.with_sink(listed.clone()).with_sink(unlisted.clone());

        let mut j = job("j1");
        j.sinks = vec!["listed".into()];
        jobs.upsert(&j).await.unwrap();
        runner.tick().await.unwrap();

        assert_eq!(listed.captured().len(), 1);
        assert!(unlisted.captured().is_empty());
    }
}
