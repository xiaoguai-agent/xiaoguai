//! The runner — picks due jobs, fires them, records `JobRun` rows,
//! writes `audit_log` entries, and pushes results to sinks.
//!
//! [`JobRunner::tick`] is the synchronous decision step (one call
//! walks the repo, fires every due *scheduled* job once, and
//! returns). [`JobRunner::fire_event`] is the event-driven step (one
//! call fires one job because a reactive source asked us to).
//! [`JobRunner::run_loop`] glues both together with `tokio::select!`
//! so the binary needs one `tokio::spawn` to drive the scheduler.
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
use std::time::Duration;

use chrono::Utc;
use thiserror::Error;
use xiaoguai_audit::AuditEntry;

use crate::audit::AuditAppender;
use crate::executor::{ExecutionOutcome, JobExecutor};
use crate::job::{JobRun, JobRunStatus, ScheduledJob};
use crate::repository::{JobRepository, JobRunRepository, RepoError};
use crate::sink::{PushPayload, PushSink};
use crate::trigger_source::{EventReceiver, TriggerEvent};

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
    ///
    /// Only fires *scheduled* jobs (cron/interval); reactive jobs are
    /// filtered out at the repository layer and only fire via
    /// [`Self::fire_event`].
    pub async fn tick(&self) -> Result<usize, RunnerError> {
        let now = Utc::now();
        let due = self
            .jobs
            .list_due(now, self.options.max_jobs_per_tick)
            .await?;
        let fired = due.len();
        for job in due {
            self.fire(job, None).await?;
        }
        Ok(fired)
    }

    /// React to one [`TriggerEvent`] — look up the job, fire it
    /// (with retries), and merge the event's `detail` into every
    /// audit row written for that fire.
    ///
    /// Missing or disabled jobs are logged + skipped: the runner
    /// keeps going. A reactive source that races with a job-delete
    /// (`DELETE FROM scheduled_jobs WHERE ...`) is normal, not a bug.
    pub async fn fire_event(&self, event: TriggerEvent) -> Result<(), RunnerError> {
        let job = match self.jobs.get(&event.job_id).await {
            Ok(j) => j,
            Err(RepoError::NotFound(_)) => {
                tracing::debug!(job_id = %event.job_id, "trigger event for unknown job");
                return Ok(());
            }
            Err(e) => return Err(RunnerError::Repository(e)),
        };
        if !job.enabled {
            tracing::debug!(job_id = %job.id, "trigger event for disabled job; skipping");
            return Ok(());
        }
        self.fire(job, Some(event.detail)).await
    }

    /// Public convenience for the operator binary: fire a single
    /// scheduled job by id, regardless of `next_fire_at`. Useful for
    /// admin-ui's "Run now" button.
    pub async fn fire_now(&self, job_id: &str) -> Result<(), RunnerError> {
        let job = self.jobs.get(job_id).await?;
        self.fire(job, None).await
    }

    /// Fire a single job once (with its retry sequence) and advance
    /// the schedule for the next fire. `extra_audit_detail`, when
    /// `Some`, is merged into every `audit_log.details` JSONB row
    /// written for this fire — reactive sources stash the changed
    /// path / webhook payload there.
    async fn fire(
        &self,
        job: ScheduledJob,
        extra_audit_detail: Option<serde_json::Value>,
    ) -> Result<(), RunnerError> {
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
            self.write_audit(
                &job,
                &inserted,
                final_status,
                error_message.as_deref(),
                extra_audit_detail.as_ref(),
            )
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

        // Advance the schedule. Reactive triggers have no next fire
        // (`next_fire_after` returns None) but we still bump
        // `last_fire_at` so the console can show "last triggered at".
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
        extra_detail: Option<&serde_json::Value>,
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
        if let Some(extra) = extra_detail {
            if !extra.is_null() {
                details["trigger"] = extra.clone();
            }
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

    /// Drive the scheduler as a long-running task: merge the
    /// scheduled-trigger timer and the reactive-trigger event
    /// channel behind one `tokio::select!`.
    ///
    /// The loop returns `Ok(())` cleanly when both arms are
    /// exhausted (event channel closed AND `tick_interval` is
    /// `None`). To shut down a runner with an active timer, drop the
    /// returned future (e.g. via `tokio::select!` against a
    /// shutdown signal in the caller).
    pub async fn run_loop(
        &self,
        mut events: EventReceiver,
        tick_interval: Option<Duration>,
    ) -> Result<(), RunnerError> {
        let mut ticker = tick_interval.map(|d| {
            let mut t = tokio::time::interval(d);
            // Skip missed ticks so a long-running fire sequence
            // doesn't queue up a burst when control returns.
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            t
        });
        let mut events_open = true;

        loop {
            tokio::select! {
                ev = async { events.recv().await }, if events_open => match ev {
                    Some(ev) => {
                        if let Err(e) = self.fire_event(ev).await {
                            tracing::error!(error = %e, "fire_event failed");
                        }
                    }
                    None => {
                        events_open = false;
                    }
                },
                () = async {
                    if let Some(t) = ticker.as_mut() {
                        t.tick().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if ticker.is_some() => {
                    if let Err(e) = self.tick().await {
                        tracing::error!(error = %e, "tick failed");
                    }
                }
            }

            if !events_open && ticker.is_none() {
                return Ok(());
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
    use crate::trigger_source::{event_channel, TriggerEvent};
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

    fn webhook_job(id: &str, route: &str) -> ScheduledJob {
        ScheduledJob::new(
            id,
            Some("tenant-x".into()),
            id,
            Trigger::webhook(route).unwrap(),
            serde_json::json!({"prompt": "hi"}),
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

    #[tokio::test]
    async fn reactive_jobs_skipped_by_tick() {
        let (runner, jobs, runs, _audit) = runner_with(Arc::new(EchoExecutor));
        let scheduled = job("scheduled");
        let reactive = webhook_job("reactive", "r1");
        jobs.upsert(&scheduled).await.unwrap();
        jobs.upsert(&reactive).await.unwrap();

        let fired = runner.tick().await.unwrap();
        assert_eq!(fired, 1, "only the scheduled job should be fired by tick");
        let runs_snap = runs.snapshot();
        assert_eq!(runs_snap.len(), 1);
        assert_eq!(runs_snap[0].job_id, "scheduled");
    }

    #[tokio::test]
    async fn fire_event_runs_reactive_job_and_merges_detail() {
        let (runner, jobs, runs, audit) = runner_with(Arc::new(EchoExecutor));
        let j = webhook_job("j1", "r1");
        jobs.upsert(&j).await.unwrap();

        let detail = serde_json::json!({"source": "webhook", "route_id": "r1"});
        let ev = TriggerEvent::new("j1").with_detail(detail.clone());
        runner.fire_event(ev).await.unwrap();

        let runs_snap = runs.snapshot();
        assert_eq!(runs_snap.len(), 1);
        assert_eq!(runs_snap[0].status, JobRunStatus::Succeeded);

        let entries = audit.snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].details["trigger"], detail);

        // last_fire_at advanced, next_fire_at stays None (reactive).
        let back = jobs.get("j1").await.unwrap();
        assert!(back.last_fire_at.is_some());
        assert!(back.next_fire_at.is_none());
    }

    #[tokio::test]
    async fn fire_event_unknown_job_is_silent() {
        let (runner, _jobs, runs, audit) = runner_with(Arc::new(EchoExecutor));
        let ev = TriggerEvent::new("nope");
        runner.fire_event(ev).await.unwrap();
        assert!(runs.snapshot().is_empty());
        assert!(audit.snapshot().is_empty());
    }

    #[tokio::test]
    async fn fire_event_disabled_job_skipped() {
        let (runner, jobs, runs, audit) = runner_with(Arc::new(EchoExecutor));
        let mut j = webhook_job("j1", "r1");
        j.enabled = false;
        jobs.upsert(&j).await.unwrap();
        runner.fire_event(TriggerEvent::new("j1")).await.unwrap();
        assert!(runs.snapshot().is_empty());
        assert!(audit.snapshot().is_empty());
    }

    #[tokio::test]
    async fn run_loop_drains_events_then_exits_when_channel_closes() {
        let (runner, jobs, runs, _audit) = runner_with(Arc::new(EchoExecutor));
        jobs.upsert(&webhook_job("j1", "r1")).await.unwrap();
        jobs.upsert(&webhook_job("j2", "r1")).await.unwrap();

        let (tx, rx) = event_channel();
        tx.send(TriggerEvent::new("j1")).await.unwrap();
        tx.send(TriggerEvent::new("j2")).await.unwrap();
        drop(tx); // close the channel → loop should exit cleanly.

        runner.run_loop(rx, None).await.unwrap();

        let mut ids: Vec<String> = runs.snapshot().into_iter().map(|r| r.job_id).collect();
        ids.sort();
        assert_eq!(ids, vec!["j1".to_string(), "j2".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_loop_interleaves_timer_and_events() {
        let (runner, jobs, runs, _audit) = runner_with(Arc::new(EchoExecutor));
        // One scheduled, one reactive.
        jobs.upsert(&job("scheduled")).await.unwrap();
        jobs.upsert(&webhook_job("reactive", "r1")).await.unwrap();

        let (tx, rx) = event_channel();
        let runner = Arc::new(runner);
        let runner_for_task = runner.clone();
        // With an active ticker, run_loop only exits via external
        // cancellation (closing the event channel doesn't stop the
        // timer arm — by design, so a deployment without reactive
        // sources still ticks). Test cancellation = task abort.
        let handle = tokio::spawn(async move {
            runner_for_task
                .run_loop(rx, Some(Duration::from_millis(50)))
                .await
                .unwrap();
        });

        // Give the timer at least one tick to fire the scheduled job.
        tokio::time::sleep(Duration::from_millis(120)).await;
        // Push one event for the reactive job.
        tx.send(TriggerEvent::new("reactive")).await.unwrap();
        // Give the event time to drain.
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Stop the loop.
        handle.abort();
        let _ = handle.await;
        drop(tx);

        let runs_snap = runs.snapshot();
        let scheduled_runs = runs_snap.iter().filter(|r| r.job_id == "scheduled").count();
        let reactive_runs = runs_snap.iter().filter(|r| r.job_id == "reactive").count();
        assert!(scheduled_runs >= 1, "timer should fire scheduled job");
        assert_eq!(reactive_runs, 1, "event fires reactive job exactly once");
    }
}
