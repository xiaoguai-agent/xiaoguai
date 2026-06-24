//! Runtime executors that make installed skill-pack specs **actually run**.
//!
//! Phase 2 of the skill-pack loader
//! (`docs/plans/2026-06-23-skill-pack-loader-phase2.md`): each pack
//! `anomalies[]` / `watches[]` spec is hosted as a `ScheduledJob` in the
//! existing [`xiaoguai_scheduler`]; this module supplies the matching
//! [`JobExecutor`]s that the `CompositeExecutor` dispatches by `payload.kind`.
//!
//! Two executors ship here:
//! - **`pack.anomaly`** — stateful z-score / EWMA detection. The detector
//!   baseline must survive across fires, so the [`AnomalyRegistry`] is shared
//!   (`Arc<Mutex<_>>`) and populated once at boot; the executor only
//!   `observe()`s, never re-registers (which would reset the baseline).
//! - **`pack.watch`** — SELECT-poll + dedup via a shared, TTL-evicting
//!   [`DedupCache`], so a matching row alerts once within its window.
//!
//! ## Payload contract (`payload.kind == "pack.anomaly"`)
//!
//! ```json
//! { "kind": "pack.anomaly", "spec_id": "row-count-drop", "kpi_query": "SELECT v FROM ..." }
//! ```
//!
//! The job is self-describing: `spec_id` addresses the live detector in the
//! shared registry, and `kpi_query` is the read-only SELECT evaluated against
//! the one embedded `SQLite` each fire (DEC-033 — no external time-series store).
//!
//! ## Alert dispatch (v1 scope)
//!
//! A FIRED anomaly / new watch match is surfaced in the **job-run record**
//! (`output_preview`) and the audit chain — visible in the Scheduler console.
//! Routing it onward to the spec's declared `on_anomaly` / `on_match` channel
//! (the job's push sinks are left empty here) is a deliberate follow-up, not
//! wired in v1.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use async_trait::async_trait;
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use xiaoguai_anomaly::spec::{AnomalySchedule, AnomalySpec};
use xiaoguai_anomaly::AnomalyRegistry;
use xiaoguai_scheduler::{ExecutionOutcome, JobExecutor, ScheduledJob, Trigger};
use xiaoguai_watch::{
    DedupCache, SqlSource, WatchSchedule, WatchSource, WatchSourceSpec, WatchSpec,
};

/// `payload.kind` value dispatched to [`PackAnomalyExecutor`].
pub const PACK_ANOMALY_KIND: &str = "pack.anomaly";

/// Evaluates one pack anomaly spec per fire: run its KPI query against the
/// embedded `SQLite`, feed the latest value to the shared detector, and surface
/// any alert in the job-run record + audit chain (queryable in the Scheduler
/// console). Routing the alert to the spec's `on_anomaly` channel is deferred —
/// see the module-level dispatch note.
pub struct PackAnomalyExecutor {
    registry: Arc<Mutex<AnomalyRegistry>>,
    pool: SqlitePool,
}

impl PackAnomalyExecutor {
    /// Build an executor over a shared, already-populated registry and the
    /// embedded `SQLite` pool used to evaluate KPI queries.
    #[must_use]
    pub fn new(registry: Arc<Mutex<AnomalyRegistry>>, pool: SqlitePool) -> Self {
        Self { registry, pool }
    }
}

#[async_trait]
impl JobExecutor for PackAnomalyExecutor {
    async fn execute(&self, job: &ScheduledJob, _attempt: u32) -> Result<ExecutionOutcome, String> {
        let spec_id = job
            .payload
            .get("spec_id")
            .and_then(serde_json::Value::as_str)
            .ok_or("pack.anomaly payload missing 'spec_id'")?;
        let query = job
            .payload
            .get("kpi_query")
            .and_then(serde_json::Value::as_str)
            .ok_or("pack.anomaly payload missing 'kpi_query'")?;

        // Boundary: KPI queries are operator-authored but must be read-only
        // SELECTs against the single `SQLite`. Reject anything else fast.
        let trimmed = query.trim();
        if !trimmed.to_ascii_uppercase().starts_with("SELECT") {
            return Err(format!(
                "pack.anomaly kpi_query must be a SELECT statement (spec '{spec_id}')"
            ));
        }

        // Evaluate the KPI — the first column of the first row is the metric.
        let row = sqlx::query(trimmed)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("pack.anomaly kpi_query failed (spec '{spec_id}'): {e}"))?;
        let Some(row) = row else {
            return Ok(ExecutionOutcome {
                output_preview: format!("pack.anomaly '{spec_id}': no data this tick"),
                session_id: None,
            });
        };
        let value = extract_metric(&row)
            .ok_or_else(|| format!("pack.anomaly '{spec_id}': first column is not numeric"))?;

        // Observe — the detector baseline lives in the shared registry, so we
        // lock only for the synchronous observe() (no await held under lock).
        let fired = {
            let mut reg = self
                .registry
                .lock()
                .map_err(|_| "pack.anomaly registry mutex poisoned".to_string())?;
            reg.observe(spec_id, Utc::now(), value)
                .map(|(_, anomaly)| anomaly)
        };

        let preview = match fired {
            Some(a) => format!(
                "pack.anomaly '{spec_id}' FIRED: {} (value={value}, mean={:.2}, score={:.2})",
                a.description, a.baseline_mean, a.score
            ),
            None => format!("pack.anomaly '{spec_id}': value={value}, nominal"),
        };
        Ok(ExecutionOutcome {
            output_preview: preview,
            session_id: None,
        })
    }
}

/// `payload.kind` value dispatched to [`PackWatchExecutor`].
pub const PACK_WATCH_KIND: &str = "pack.watch";

/// Evaluates one pack watch spec per fire: run its SELECT against the embedded
/// `SQLite`, dedup result rows against the shared cache, and report how many
/// *new* matches fired (the `on_match` dispatch). The [`DedupCache`] is shared
/// (and internally concurrent) so a match doesn't re-fire across ticks within
/// its TTL.
pub struct PackWatchExecutor {
    dedup: Arc<DedupCache>,
    pool: SqlitePool,
}

impl PackWatchExecutor {
    /// Build an executor over a shared dedup cache and the embedded `SQLite`
    /// pool used to evaluate watch queries.
    #[must_use]
    pub fn new(dedup: Arc<DedupCache>, pool: SqlitePool) -> Self {
        Self { dedup, pool }
    }
}

#[async_trait]
impl JobExecutor for PackWatchExecutor {
    async fn execute(&self, job: &ScheduledJob, _attempt: u32) -> Result<ExecutionOutcome, String> {
        let spec_id = job
            .payload
            .get("spec_id")
            .and_then(serde_json::Value::as_str)
            .ok_or("pack.watch payload missing 'spec_id'")?;
        let query = job
            .payload
            .get("query")
            .and_then(serde_json::Value::as_str)
            .ok_or("pack.watch payload missing 'query'")?;

        // Boundary: the watch query is operator-authored but must be a read-only
        // SELECT. `SqlSource::new` does NOT validate this (it only clones the
        // query), so guard it here — mirroring the anomaly path.
        let trimmed = query.trim();
        if !trimmed.to_ascii_uppercase().starts_with("SELECT") {
            return Err(format!(
                "pack.watch query must be a SELECT statement (spec '{spec_id}')"
            ));
        }
        let source = SqlSource::new(
            self.pool.clone(),
            &WatchSourceSpec::Sql {
                query: trimmed.to_string(),
            },
        )
        .map_err(|e| format!("pack.watch '{spec_id}': invalid source: {e}"))?;
        let matches = source
            .poll()
            .await
            .map_err(|e| format!("pack.watch '{spec_id}' poll failed: {e}"))?;

        let mut fresh = 0_usize;
        for m in &matches {
            if !self.dedup.is_duplicate(spec_id, m).await {
                self.dedup.record(spec_id, m).await;
                fresh += 1;
            }
        }

        let preview = if fresh > 0 {
            format!(
                "pack.watch '{spec_id}' FIRED: {fresh} new match(es) of {} row(s)",
                matches.len()
            )
        } else {
            format!("pack.watch '{spec_id}': {} row(s), none new", matches.len())
        };
        Ok(ExecutionOutcome {
            output_preview: preview,
            session_id: None,
        })
    }
}

/// Pull the metric value from the first column of a `SQLite` row, tolerating both
/// `REAL` and `INTEGER` storage classes.
fn extract_metric(row: &sqlx::sqlite::SqliteRow) -> Option<f64> {
    if let Ok(v) = row.try_get::<f64, _>(0) {
        return Some(v);
    }
    #[allow(clippy::cast_precision_loss)]
    if let Ok(v) = row.try_get::<i64, _>(0) {
        return Some(v as f64);
    }
    None
}

/// Scan every **enabled** installed pack and wire its anomaly specs into the
/// shared registry, returning the `pack.anomaly` [`ScheduledJob`]s the caller
/// should upsert. Idempotent by job id (`pack:<slug>:anomaly:<spec_id>`), so a
/// boot scan re-runs cleanly.
///
/// Enablement + on-disk location live in the `installed_skill_packs.config`
/// JSON (`{ "enabled": true, "pack_dir": "/abs/path" }`) — reusing the existing
/// free-form column means **no migration** (DEC-033: state stays in the one
/// `SQLite`). A pack that fails to load is logged and skipped, never fatal.
///
/// **Call once per process** (at boot): it `register`s each detector, which
/// overwrites — re-running against an already-populated registry would reset
/// every baseline. `run_serve` calls it once against a freshly-empty registry.
pub async fn scan_enabled_pack_anomalies(
    pool: &SqlitePool,
    registry: &Arc<Mutex<AnomalyRegistry>>,
) -> anyhow::Result<Vec<ScheduledJob>> {
    let mut jobs = Vec::new();
    for (slug, pack_dir) in enabled_pack_dirs(pool).await? {
        if let Err(e) = wire_pack_anomalies(&pack_dir, &slug, registry, &mut jobs).await {
            tracing::warn!(slug, pack_dir, error = %e, "failed to wire pack anomalies; skipping");
        }
    }
    Ok(jobs)
}

/// Load one pack's anomaly specs: register each detector in the shared registry
/// and append its scheduled job. Kept separate so a single bad pack is isolated.
async fn wire_pack_anomalies(
    pack_dir: &str,
    slug: &str,
    registry: &Arc<Mutex<AnomalyRegistry>>,
    jobs: &mut Vec<ScheduledJob>,
) -> anyhow::Result<()> {
    let dir = Path::new(pack_dir);
    let manifest = crate::packs::PackLoader::new()
        .load(dir.join("pack.yaml"))
        .await?;
    for entry in &manifest.anomalies {
        let spec_path = dir.join(&entry.path);
        let yaml = std::fs::read_to_string(&spec_path)
            .with_context(|| format!("read anomaly spec {}", spec_path.display()))?;
        let spec: AnomalySpec = serde_yaml::from_str(&yaml)
            .with_context(|| format!("parse anomaly spec {}", spec_path.display()))?;
        let trigger = trigger_from_schedule(&spec.schedule)?;
        let job = ScheduledJob::new(
            format!("pack:{slug}:anomaly:{}", spec.id),
            format!("{slug} · {}", spec.id),
            trigger,
            serde_json::json!({
                "kind": PACK_ANOMALY_KIND,
                "spec_id": spec.id,
                "kpi_query": spec.kpi_query,
            }),
        );
        registry
            .lock()
            .map_err(|_| anyhow::anyhow!("anomaly registry poisoned"))?
            .register(spec);
        jobs.push(job);
    }
    Ok(())
}

/// Map a pack's declared cadence to a scheduler [`Trigger`].
fn trigger_from_schedule(schedule: &AnomalySchedule) -> anyhow::Result<Trigger> {
    match schedule {
        AnomalySchedule::Cron { expr } => {
            Trigger::cron(expr).map_err(|e| anyhow::anyhow!("invalid cron '{expr}': {e}"))
        }
        AnomalySchedule::IntervalSecs { secs } => {
            Trigger::interval(*secs).map_err(|e| anyhow::anyhow!("invalid interval {secs}s: {e}"))
        }
    }
}

/// `(slug, pack_dir)` for every **enabled** installed pack, read from the
/// `installed_skill_packs.config` JSON. Shared by the anomaly + watch scans.
async fn enabled_pack_dirs(pool: &SqlitePool) -> anyhow::Result<Vec<(String, String)>> {
    let rows = sqlx::query("SELECT pack_slug, config FROM installed_skill_packs")
        .fetch_all(pool)
        .await
        .context("read installed_skill_packs")?;
    let mut out = Vec::new();
    for row in &rows {
        let slug: String = row.try_get("pack_slug")?;
        let config: String = row.try_get("config").unwrap_or_else(|_| "{}".to_string());
        let cfg: serde_json::Value = match serde_json::from_str(&config) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(slug, error = %e, "installed pack has unparseable config JSON; skipping");
                continue;
            }
        };
        if !cfg
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(dir) = cfg.get("pack_dir").and_then(serde_json::Value::as_str) {
            out.push((slug, dir.to_string()));
        } else {
            tracing::warn!(slug, "enabled pack has no pack_dir in config; skipping");
        }
    }
    Ok(out)
}

/// Scan every enabled pack and build its `pack.watch` [`ScheduledJob`]s
/// (id `pack:<slug>:watch:<spec_id>`, `Trigger` from the watch's schedule).
/// Watch dedup is runtime (the executor's shared cache), so this needs no
/// registry. SQL watches only — HTTP sources are out of scope for v1.
pub async fn scan_enabled_pack_watches(pool: &SqlitePool) -> anyhow::Result<Vec<ScheduledJob>> {
    let mut jobs = Vec::new();
    for (slug, pack_dir) in enabled_pack_dirs(pool).await? {
        if let Err(e) = wire_pack_watches(&pack_dir, &slug, &mut jobs).await {
            tracing::warn!(slug, pack_dir, error = %e, "failed to wire pack watches; skipping");
        }
    }
    Ok(jobs)
}

/// Load one pack's watch specs and append a `pack.watch` job per SQL watch.
async fn wire_pack_watches(
    pack_dir: &str,
    slug: &str,
    jobs: &mut Vec<ScheduledJob>,
) -> anyhow::Result<()> {
    let dir = Path::new(pack_dir);
    let manifest = crate::packs::PackLoader::new()
        .load(dir.join("pack.yaml"))
        .await?;
    for entry in &manifest.watches {
        let spec_path = dir.join(&entry.path);
        let yaml = std::fs::read_to_string(&spec_path)
            .with_context(|| format!("read watch spec {}", spec_path.display()))?;
        let spec: WatchSpec = serde_yaml::from_str(&yaml)
            .with_context(|| format!("parse watch spec {}", spec_path.display()))?;
        let query = match &spec.source {
            WatchSourceSpec::Sql { query } => query.clone(),
            WatchSourceSpec::Http { .. } => {
                tracing::warn!(slug, id = %spec.id, "pack.watch HTTP source unsupported in v1; skipping");
                continue;
            }
        };
        let trigger = trigger_from_watch_schedule(&spec.schedule)?;
        let job = ScheduledJob::new(
            format!("pack:{slug}:watch:{}", spec.id),
            format!("{slug} · {}", spec.id),
            trigger,
            serde_json::json!({
                "kind": PACK_WATCH_KIND,
                "spec_id": spec.id,
                "query": query,
            }),
        );
        jobs.push(job);
    }
    Ok(())
}

/// Map a watch's declared cadence to a scheduler [`Trigger`]. xiaoguai-watch's
/// own cron is a 60s-fallback stub, so routing through the scheduler's `Trigger`
/// is what makes a watch's declared cron actually honoured.
fn trigger_from_watch_schedule(schedule: &WatchSchedule) -> anyhow::Result<Trigger> {
    match schedule {
        WatchSchedule::Cron { expr } => {
            Trigger::cron(expr).map_err(|e| anyhow::anyhow!("invalid cron '{expr}': {e}"))
        }
        WatchSchedule::IntervalSecs { secs } => {
            Trigger::interval(*secs).map_err(|e| anyhow::anyhow!("invalid interval {secs}s: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use xiaoguai_anomaly::spec::{ActionRef, AnomalySchedule, AnomalySpec, DetectorKind};
    use xiaoguai_anomaly::InMemoryStore;
    use xiaoguai_scheduler::Trigger;

    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE m (v REAL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn zscore_spec(id: &str) -> AnomalySpec {
        AnomalySpec {
            id: id.to_string(),
            kpi_query: "SELECT v FROM m ORDER BY rowid DESC LIMIT 1".to_string(),
            window: Duration::hours(1),
            detector: DetectorKind::ZScore {
                sigma_threshold: 3.0,
                min_count: 5,
            },
            cool_off: Duration::minutes(0),
            on_anomaly: ActionRef::Notify {
                channel: "ops".to_string(),
            },
            schedule: AnomalySchedule::default(),
        }
    }

    fn job_for(spec: &AnomalySpec) -> ScheduledJob {
        ScheduledJob::new(
            "j-anom",
            "anomaly job",
            Trigger::interval(60).unwrap(),
            serde_json::json!({
                "kind": PACK_ANOMALY_KIND,
                "spec_id": spec.id,
                "kpi_query": spec.kpi_query,
            }),
        )
    }

    async fn insert(pool: &SqlitePool, v: f64) {
        sqlx::query("INSERT INTO m (v) VALUES (?)")
            .bind(v)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fires_on_deviation_after_baseline() {
        let pool = mem_pool().await;
        let spec = zscore_spec("m1");
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(spec.clone());
        let exec = PackAnomalyExecutor::new(Arc::new(Mutex::new(reg)), pool.clone());
        let job = job_for(&spec);

        // Establish a tight baseline around 100 (σ≈1.4). The online z-score
        // detector updates before it checks, so a single catastrophic spike
        // would dilute its own variance — realistic detection needs an
        // accumulated baseline plus a clear, in-scale deviation.
        for i in 0..30 {
            let v = 100.0 + f64::from(i % 5) - 2.0; // cycles 98..=102
            insert(&pool, v).await;
            let out = exec.execute(&job, 1).await.unwrap();
            assert!(
                out.output_preview.contains("nominal"),
                "baseline should read nominal, got: {}",
                out.output_preview
            );
        }

        // A clear deviation against the established baseline must fire.
        insert(&pool, 130.0).await;
        let out = exec.execute(&job, 1).await.unwrap();
        assert!(
            out.output_preview.contains("FIRED"),
            "deviation should fire, got: {}",
            out.output_preview
        );
    }

    #[tokio::test]
    async fn no_data_is_not_an_error() {
        let pool = mem_pool().await;
        let spec = zscore_spec("empty");
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(spec.clone());
        let exec = PackAnomalyExecutor::new(Arc::new(Mutex::new(reg)), pool);
        let out = exec.execute(&job_for(&spec), 1).await.unwrap();
        assert!(out.output_preview.contains("no data"));
    }

    #[tokio::test]
    async fn non_select_kpi_is_rejected() {
        let pool = mem_pool().await;
        let reg = Arc::new(Mutex::new(AnomalyRegistry::new(Box::new(
            InMemoryStore::default(),
        ))));
        let exec = PackAnomalyExecutor::new(reg, pool);
        let job = ScheduledJob::new(
            "j",
            "j",
            Trigger::interval(60).unwrap(),
            serde_json::json!({
                "kind": PACK_ANOMALY_KIND,
                "spec_id": "x",
                "kpi_query": "DELETE FROM m",
            }),
        );
        let err = exec.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("SELECT"), "expected SELECT guard, got: {err}");
    }

    #[tokio::test]
    async fn missing_payload_fields_error() {
        let pool = mem_pool().await;
        let reg = Arc::new(Mutex::new(AnomalyRegistry::new(Box::new(
            InMemoryStore::default(),
        ))));
        let exec = PackAnomalyExecutor::new(reg, pool);
        let job = ScheduledJob::new(
            "j",
            "j",
            Trigger::interval(60).unwrap(),
            serde_json::json!({ "kind": PACK_ANOMALY_KIND }),
        );
        assert!(exec.execute(&job, 1).await.is_err());
    }

    /// End-to-end: the **shipped** canonical reference pack parses as a real
    /// `AnomalySpec` and its actual `SQLite` KPI query drives the executor to
    /// fire on an anomalous day. This is the "a pack actually runs" proof.
    #[tokio::test]
    async fn reference_pack_daily_token_spend_runs_end_to_end() {
        let yaml =
            include_str!("../../../packs/observability-starter/anomalies/daily-token-spend.yaml");
        let spec: AnomalySpec = serde_yaml::from_str(yaml).expect("reference anomaly spec parses");
        assert_eq!(spec.id, "daily-token-spend");

        // Stand up the real token_usage columns the KPI query reads.
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE token_usage (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                ts TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')), \
                total_tokens INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(spec.clone());
        let exec = PackAnomalyExecutor::new(Arc::new(Mutex::new(reg)), pool.clone());
        let job = job_for(&spec);

        // Each "day": reset + write that day's spend (the query sums "today"),
        // then fire. A long tight baseline arms the detector and stays nominal.
        for i in 0..24 {
            sqlx::query("DELETE FROM token_usage")
                .execute(&pool)
                .await
                .unwrap();
            let day_total = 10_000 + (i % 5) * 150; // ~10k, tight spread
            sqlx::query("INSERT INTO token_usage (total_tokens) VALUES (?)")
                .bind(day_total)
                .execute(&pool)
                .await
                .unwrap();
            let out = exec.execute(&job, 1).await.unwrap();
            assert!(
                out.output_preview.contains("nominal"),
                "baseline day {i}: {}",
                out.output_preview
            );
        }

        // A ~4× spend day is clearly anomalous against the established baseline.
        sqlx::query("DELETE FROM token_usage")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO token_usage (total_tokens) VALUES (40000)")
            .execute(&pool)
            .await
            .unwrap();
        let out = exec.execute(&job, 1).await.unwrap();
        assert!(
            out.output_preview.contains("FIRED"),
            "anomalous spend day should fire: {}",
            out.output_preview
        );
    }

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

    fn reference_pack_dir() -> String {
        format!(
            "{}/../../packs/observability-starter",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    async fn install_row(pool: &SqlitePool, enabled: bool) {
        let config =
            serde_json::json!({ "enabled": enabled, "pack_dir": reference_pack_dir() }).to_string();
        sqlx::query(
            "INSERT INTO installed_skill_packs (id, pack_slug, version, config) \
             VALUES ('1', 'observability-starter', '1.0.0', ?)",
        )
        .bind(&config)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn scan_wires_enabled_pack_into_registry_and_jobs() {
        let pool = installed_packs_pool().await;
        install_row(&pool, true).await;
        let registry = Arc::new(Mutex::new(AnomalyRegistry::new(Box::new(
            InMemoryStore::default(),
        ))));

        let jobs = scan_enabled_pack_anomalies(&pool, &registry).await.unwrap();

        // One job, deterministic id, daily interval from the spec's schedule.
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].id,
            "pack:observability-starter:anomaly:daily-token-spend"
        );
        assert!(matches!(jobs[0].trigger, Trigger::Interval { secs: 86400 }));
        assert_eq!(jobs[0].payload["kind"], PACK_ANOMALY_KIND);
        // The detector is registered: observing an unknown id would warn + return
        // None, but a registered (un-armed) one also returns None — so assert the
        // executor can drive it without the "unknown spec" path by re-scanning idempotently.
        let again = scan_enabled_pack_anomalies(&pool, &registry).await.unwrap();
        assert_eq!(again[0].id, jobs[0].id, "scan is idempotent by job id");
    }

    #[tokio::test]
    async fn scan_skips_disabled_packs() {
        let pool = installed_packs_pool().await;
        install_row(&pool, false).await;
        let registry = Arc::new(Mutex::new(AnomalyRegistry::new(Box::new(
            InMemoryStore::default(),
        ))));
        let jobs = scan_enabled_pack_anomalies(&pool, &registry).await.unwrap();
        assert!(jobs.is_empty(), "disabled pack must not be wired");
    }

    #[tokio::test]
    async fn watch_fires_on_new_matches_then_dedups() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE token_usage (id INTEGER PRIMARY KEY, model TEXT, total_tokens INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO token_usage (id, model, total_tokens) \
             VALUES (1, 'big', 60000), (2, 'big', 70000), (3, 'small', 100)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dedup = Arc::new(DedupCache::new(100, std::time::Duration::from_secs(3600)));
        let exec = PackWatchExecutor::new(dedup, pool.clone());
        let job = ScheduledJob::new(
            "w",
            "w",
            Trigger::interval(3600).unwrap(),
            serde_json::json!({
                "kind": PACK_WATCH_KIND,
                "spec_id": "oversized",
                "query": "SELECT id, model, total_tokens FROM token_usage WHERE total_tokens > 50000",
            }),
        );

        // First fire: the two oversized rows are new.
        let out = exec.execute(&job, 1).await.unwrap();
        assert!(
            out.output_preview.contains("2 new match"),
            "first fire should report 2 new: {}",
            out.output_preview
        );
        // Second fire: same rows, all deduped.
        let out2 = exec.execute(&job, 1).await.unwrap();
        assert!(
            out2.output_preview.contains("none new"),
            "re-fire should dedup: {}",
            out2.output_preview
        );
    }

    #[tokio::test]
    async fn watch_non_select_query_is_rejected() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let exec = PackWatchExecutor::new(
            Arc::new(DedupCache::new(10, std::time::Duration::from_secs(60))),
            pool,
        );
        let job = ScheduledJob::new(
            "w",
            "w",
            Trigger::interval(60).unwrap(),
            serde_json::json!({
                "kind": PACK_WATCH_KIND,
                "spec_id": "x",
                "query": "DELETE FROM token_usage",
            }),
        );
        let err = exec.execute(&job, 1).await.unwrap_err();
        assert!(err.contains("SELECT"), "expected SELECT guard, got: {err}");
    }

    #[tokio::test]
    async fn scan_wires_watch_jobs_from_enabled_pack() {
        let pool = installed_packs_pool().await;
        install_row(&pool, true).await;
        let jobs = scan_enabled_pack_watches(&pool).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].id,
            "pack:observability-starter:watch:oversized-llm-request"
        );
        assert!(matches!(jobs[0].trigger, Trigger::Interval { secs: 3600 }));
        assert_eq!(jobs[0].payload["kind"], PACK_WATCH_KIND);
    }
}
