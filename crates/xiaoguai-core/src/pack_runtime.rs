//! Runtime executors that make installed skill-pack specs **actually run**.
//!
//! Phase 2 of the skill-pack loader
//! (`docs/plans/2026-06-23-skill-pack-loader-phase2.md`): each pack
//! `anomalies[]` / `watches[]` spec is hosted as a `ScheduledJob` in the
//! existing [`xiaoguai_scheduler`]; this module supplies the matching
//! [`JobExecutor`]s that the `CompositeExecutor` dispatches by `payload.kind`.
//!
//! This slice ships the **anomaly** path only. The detector baseline is
//! stateful and must survive across fires, so the [`AnomalyRegistry`] is shared
//! (`Arc<Mutex<_>>`) and populated once at boot/install — the executor only
//! `observe()`s, it never re-registers (which would reset the baseline).
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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use xiaoguai_anomaly::AnomalyRegistry;
use xiaoguai_scheduler::{ExecutionOutcome, JobExecutor, ScheduledJob};

/// `payload.kind` value dispatched to [`PackAnomalyExecutor`].
pub const PACK_ANOMALY_KIND: &str = "pack.anomaly";

/// Evaluates one pack anomaly spec per fire: run its KPI query against the
/// embedded `SQLite`, feed the latest value to the shared detector, and surface
/// any alert as the run's output preview (which the scheduler's sinks push).
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use xiaoguai_anomaly::spec::{ActionRef, AnomalySpec, DetectorKind};
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
}
