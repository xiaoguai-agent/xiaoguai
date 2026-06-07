//! `xiaoguai schedule {create,list,show,pause,resume,delete,run-now}` —
//! manage the scheduler's cron jobs from the CLI.
//!
//! Write paths (create / pause / resume / delete) go straight at the local
//! `SQLite` store through the scheduler repositories — same pattern as
//! `provider` / `mcp` — because the server's `JobRunner` polls the same
//! tables on every tick, so changes are picked up without a restart. Every
//! write also appends a `schedule.*` row to the HMAC audit chain via the
//! scheduler's [`AuditAppender`] seam (matching the runner's audit-first
//! contract).
//!
//! `run-now` is the exception: an out-of-band fire needs the live
//! `JobRunner` inside the server process, so it goes over REST
//! (`POST /v1/admin/scheduler/jobs/:id/fire-now`) like `hotl` / `outcomes`.
//!
//! Functions take repository / appender trait objects so unit tests can use
//! the scheduler crate's in-memory implementations.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_audit::{AuditEntry, OWNER_TENANT_ID};
use xiaoguai_scheduler::{
    AuditAppender, JobRepository, JobRun, JobRunRepository, RepoError, ScheduledJob, Trigger,
};

/// [`AuditAppender`] over the real HMAC-chained [`SqliteAuditSink`] — the
/// CLI-side twin of `xiaoguai-core`'s (private) scheduler audit shim.
pub struct SinkAuditAppender {
    sink: Arc<SqliteAuditSink>,
}

impl SinkAuditAppender {
    #[must_use]
    pub fn new(sink: Arc<SqliteAuditSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl AuditAppender for SinkAuditAppender {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.sink
            .append(entry)
            .await
            .map(|_stored| ())
            .map_err(|e| e.to_string())
    }
}

/// Audit actor for CLI-driven schedule administration. The CLI runs under
/// the owner's implicit authority (DEC-033 single-user).
const AUDIT_ACTOR: &str = "cli:owner";

/// Upper bound on rows scanned when resolving a short-id prefix.
const RESOLVE_SCAN_LIMIT: usize = 10_000;

/// Run-history rows shown by `schedule show`.
const SHOW_RUN_LIMIT: usize = 10;

/// Characters of the job id shown in the `list` table. Long enough to be
/// unique in practice (`job_` + 8 hex); `resolve` accepts any unique prefix.
const SHORT_ID_LEN: usize = 12;

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CreateArgs {
    pub name: String,
    /// 6-field cron expression (sec min hour day-of-month month day-of-week),
    /// evaluated in UTC. Validated eagerly via the scheduler's cron parser.
    pub cron: String,
    /// Agent prompt the executor runs on each fire — lands in
    /// `payload.prompt`, the contract `RuntimeJobExecutor` reads.
    pub prompt: String,
    pub description: Option<String>,
    /// Push-sink ids for results, e.g. `feishu:chat-x`, `inbox:owner`.
    /// Empty = no push (results land in the run history + audit log only).
    pub sinks: Vec<String>,
}

/// Validate + persist a new cron job, primed with its first `next_fire_at`
/// so it fires at the scheduled time rather than immediately.
///
/// # Errors
/// Returns an error if `--name` / `--prompt` are blank, the cron expression
/// does not parse (teaching error with the 6-field format + an example), or
/// the repository / audit write fails.
pub async fn create(
    repo: &dyn JobRepository,
    audit: &dyn AuditAppender,
    args: CreateArgs,
) -> Result<ScheduledJob> {
    let name = args.name.trim();
    if name.is_empty() {
        bail!("--name must not be empty");
    }
    if args.prompt.trim().is_empty() {
        bail!("--prompt must not be empty");
    }
    let trigger = Trigger::cron(args.cron.trim()).map_err(|e| {
        anyhow!(
            "invalid cron expression '{}': {e}. The scheduler uses 6-field cron \
             (sec min hour day-of-month month day-of-week, evaluated in UTC) — \
             e.g. '0 0 8 * * *' fires every day at 08:00:00 UTC.",
            args.cron
        )
    })?;

    let mut job = ScheduledJob::new(
        format!("job_{}", uuid::Uuid::new_v4().simple()),
        name,
        trigger,
        serde_json::json!({ "prompt": args.prompt }),
    );
    job.description = args
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    job.sinks = args.sinks;
    // Prime the schedule so the runner's "no next_fire_at = fire immediately"
    // rule doesn't trigger an unscheduled first run.
    job.next_fire_at = job
        .trigger
        .next_fire_after(Utc::now())
        .map_err(|e| anyhow!("compute next fire: {e}"))?;

    repo.upsert(&job).await.map_err(repo_err)?;
    append_audit(
        audit,
        "schedule.create",
        &job.id,
        serde_json::json!({ "name": job.name, "cron": args.cron.trim() }),
    )
    .await?;
    Ok(job)
}

// ---------------------------------------------------------------------------
// list + show
// ---------------------------------------------------------------------------

/// One row of the `schedule list` table: the job plus its most recent run
/// (if any) for the LAST RESULT column.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub job: ScheduledJob,
    pub last_run: Option<JobRun>,
}

/// List jobs (enabled and paused) with each job's latest run.
///
/// # Errors
/// Returns an error if a repository query fails.
pub async fn list(
    repo: &dyn JobRepository,
    runs: &dyn JobRunRepository,
    limit: usize,
) -> Result<Vec<JobRow>> {
    let jobs = repo.list_all(limit).await.map_err(repo_err)?;
    let mut out = Vec::with_capacity(jobs.len());
    for job in jobs {
        let last_run = runs
            .list_for_job(&job.id, 1)
            .await
            .map_err(repo_err)?
            .into_iter()
            .next();
        out.push(JobRow { job, last_run });
    }
    Ok(out)
}

/// Fetch one job (by id or unique prefix) plus its recent run history.
///
/// # Errors
/// Returns an error if the id does not resolve or a repository query fails.
pub async fn show(
    repo: &dyn JobRepository,
    runs: &dyn JobRunRepository,
    id: &str,
) -> Result<(ScheduledJob, Vec<JobRun>)> {
    let job = resolve(repo, id).await?;
    let history = runs
        .list_for_job(&job.id, SHOW_RUN_LIMIT)
        .await
        .map_err(repo_err)?;
    Ok((job, history))
}

/// Resolve a job by exact id or unique id prefix (the `list` table shows
/// short ids).
///
/// # Errors
/// Returns a teaching error when nothing matches (points at
/// `xiaoguai schedule list`) or when the prefix is ambiguous (lists the
/// candidates).
pub async fn resolve(repo: &dyn JobRepository, id_or_prefix: &str) -> Result<ScheduledJob> {
    let needle = id_or_prefix.trim();
    if needle.is_empty() {
        bail!("job id must not be empty — run `xiaoguai schedule list` to see ids");
    }
    if let Ok(job) = repo.get(needle).await {
        return Ok(job);
    }
    let all = repo.list_all(RESOLVE_SCAN_LIMIT).await.map_err(repo_err)?;
    let matches: Vec<&ScheduledJob> = all.iter().filter(|j| j.id.starts_with(needle)).collect();
    match matches.as_slice() {
        [] => {
            bail!("no scheduled job matches '{needle}' — run `xiaoguai schedule list` to see ids")
        }
        [one] => Ok((*one).clone()),
        many => {
            let candidates: Vec<String> = many
                .iter()
                .map(|j| format!("{} ({})", j.id, j.name))
                .collect();
            bail!(
                "'{needle}' is ambiguous — it matches {} jobs:\n  {}\nUse a longer prefix or the full id.",
                many.len(),
                candidates.join("\n  ")
            )
        }
    }
}

// ---------------------------------------------------------------------------
// pause / resume / delete
// ---------------------------------------------------------------------------

/// Pause (`enabled = false`) or resume (`enabled = true`) a job. Resuming
/// recomputes `next_fire_at` from now so a stale past timestamp doesn't
/// trigger an immediate catch-up fire.
///
/// # Errors
/// Returns an error if the id does not resolve or the repository / audit
/// write fails.
pub async fn set_enabled(
    repo: &dyn JobRepository,
    audit: &dyn AuditAppender,
    id: &str,
    enabled: bool,
) -> Result<ScheduledJob> {
    let mut job = resolve(repo, id).await?;
    job.enabled = enabled;
    job.updated_at = Utc::now();
    if enabled {
        job.next_fire_at = job
            .trigger
            .next_fire_after(Utc::now())
            .map_err(|e| anyhow!("compute next fire: {e}"))?;
    }
    repo.upsert(&job).await.map_err(repo_err)?;
    let action = if enabled {
        "schedule.resume"
    } else {
        "schedule.pause"
    };
    append_audit(
        audit,
        action,
        &job.id,
        serde_json::json!({ "name": job.name }),
    )
    .await?;
    Ok(job)
}

/// Delete a job (run history cascades at the SQL layer). Returns the
/// deleted job so the caller can print its name. Confirmation is the
/// caller's responsibility (`main.rs` prompts unless `--yes`).
///
/// # Errors
/// Returns an error if the id does not resolve or the repository / audit
/// write fails.
pub async fn delete(
    repo: &dyn JobRepository,
    audit: &dyn AuditAppender,
    id: &str,
) -> Result<ScheduledJob> {
    let job = resolve(repo, id).await?;
    repo.delete(&job.id).await.map_err(repo_err)?;
    append_audit(
        audit,
        "schedule.delete",
        &job.id,
        serde_json::json!({ "name": job.name }),
    )
    .await?;
    Ok(job)
}

// ---------------------------------------------------------------------------
// run-now (REST — the fire needs the live JobRunner in the server process)
// ---------------------------------------------------------------------------

/// Fire a job immediately via the running server's admin API. `job_id` must
/// be the full id (callers resolve prefixes against the local store first).
///
/// # Errors
/// Returns a teaching error when the server is unreachable, the job is
/// unknown to the server (404), or the scheduler isn't wired (503).
pub async fn run_now(server: &str, job_id: &str) -> Result<()> {
    let url = format!(
        "{}/v1/admin/scheduler/jobs/{job_id}/fire-now",
        server.trim_end_matches('/')
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .send()
        .await
        .with_context(|| {
            format!("could not reach {server} — is `xiaoguai serve` running there?")
        })?;
    let status = resp.status();
    match status.as_u16() {
        s if status.is_success() => {
            let _ = s;
            Ok(())
        }
        404 => bail!(
            "the server does not know job '{job_id}' — run `xiaoguai schedule list` \
             and check both CLI and server point at the same database"
        ),
        503 => bail!(
            "the server's scheduler is not wired (503) — restart `xiaoguai serve` \
             and check its startup logs"
        ),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            bail!("API returned {status}: {body}")
        }
    }
}

// ---------------------------------------------------------------------------
// formatting (pure, unit-testable)
// ---------------------------------------------------------------------------

/// Render the `schedule list` table.
#[must_use]
pub fn format_table(rows: &[JobRow]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<14} {:<20} {:<28} {:<17} {:<8} LAST RESULT",
        "ID", "NAME", "TRIGGER", "NEXT FIRE", "STATUS"
    );
    for row in rows {
        let j = &row.job;
        let status = if j.enabled { "active" } else { "paused" };
        let last = row.last_run.as_ref().map_or("-", |r| r.status.as_str());
        let _ = writeln!(
            out,
            "{:<14} {:<20} {:<28} {:<17} {:<8} {last}",
            short_id(&j.id),
            truncate(&j.name, 20),
            truncate(&trigger_summary(&j.trigger), 28),
            format_ts_opt(j.next_fire_at.filter(|_| j.enabled)),
            status,
        );
    }
    out
}

/// Render the `schedule show` detail view.
#[must_use]
pub fn format_detail(job: &ScheduledJob, runs: &[JobRun]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "id:           {}", job.id);
    let _ = writeln!(out, "name:         {}", job.name);
    if let Some(d) = &job.description {
        let _ = writeln!(out, "description:  {d}");
    }
    let _ = writeln!(out, "trigger:      {}", trigger_summary(&job.trigger));
    let _ = writeln!(
        out,
        "status:       {}",
        if job.enabled { "active" } else { "paused" }
    );
    let _ = writeln!(out, "next fire:    {}", format_ts_opt(job.next_fire_at));
    let _ = writeln!(out, "last fire:    {}", format_ts_opt(job.last_fire_at));
    if let Some(prompt) = job
        .payload
        .get("prompt")
        .and_then(serde_json::Value::as_str)
    {
        let _ = writeln!(out, "prompt:       {prompt}");
    }
    if !job.sinks.is_empty() {
        let _ = writeln!(out, "sinks:        {}", job.sinks.join(", "));
    }
    let _ = writeln!(out, "created:      {}", format_ts(job.created_at));
    let _ = writeln!(out, "updated:      {}", format_ts(job.updated_at));

    if runs.is_empty() {
        let _ = writeln!(out, "\nno runs recorded yet");
    } else {
        let _ = writeln!(out, "\nrecent runs (newest first):");
        let _ = writeln!(
            out,
            "  {:<8} {:<10} {:<8} {:<17} OUTPUT",
            "RUN", "STATUS", "ATTEMPT", "FINISHED"
        );
        for r in runs {
            let output = r
                .error_message
                .as_deref()
                .or(r.output_preview.as_deref())
                .unwrap_or("-");
            let _ = writeln!(
                out,
                "  {:<8} {:<10} {:<8} {:<17} {}",
                r.id,
                r.status.as_str(),
                r.attempt,
                format_ts_opt(r.finished_at),
                truncate(output, 60),
            );
        }
    }
    out
}

/// Short display form of a job id for the list table.
#[must_use]
pub fn short_id(id: &str) -> &str {
    let cut = id
        .char_indices()
        .nth(SHORT_ID_LEN)
        .map_or(id.len(), |(i, _)| i);
    &id[..cut]
}

fn trigger_summary(t: &Trigger) -> String {
    match t {
        Trigger::Cron { expr } => format!("cron `{expr}` (UTC)"),
        Trigger::Interval { secs } => format!("every {secs}s"),
        Trigger::FileWatch { path } => format!("watch `{path}`"),
        Trigger::Webhook { route_id } => format!("webhook `{route_id}`"),
        Trigger::GitPush { repo_url, branch } => format!("git push `{repo_url}`@`{branch}`"),
        Trigger::DbPoll { query } => format!("db poll `{query}`"),
        Trigger::Proactive { interval_secs, .. } => format!("proactive every {interval_secs}s"),
    }
}

fn format_ts(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%MZ").to_string()
}

fn format_ts_opt(ts: Option<DateTime<Utc>>) -> String {
    ts.map_or_else(|| "-".to_string(), format_ts)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

#[allow(clippy::needless_pass_by_value)] // map_err passes the error by value
fn repo_err(e: RepoError) -> anyhow::Error {
    anyhow!("scheduler store: {e}")
}

/// Append a `schedule.*` row to the audit chain. A failed append is a hard
/// error, matching the runner's audit-first contract (`RunnerError::Audit`).
async fn append_audit(
    audit: &dyn AuditAppender,
    action: &str,
    job_id: &str,
    details: serde_json::Value,
) -> Result<()> {
    audit
        .append(AuditEntry {
            ts: Utc::now(),
            tenant_id: OWNER_TENANT_ID.to_string(),
            actor: AUDIT_ACTOR.to_string(),
            action: action.to_string(),
            resource: Some(job_id.to_string()),
            details,
        })
        .await
        .map_err(|e| anyhow!("audit append for {action}: {e}"))
}
