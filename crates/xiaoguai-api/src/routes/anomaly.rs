//! `POST /v1/anomaly/test` + `POST /v1/anomaly/run` — anomaly-monitor REST
//! surface backing the `xiaoguai anomaly` CLI.
//!
//! - **`/test`** back-tests an [`AnomalySpec`] against an inline CSV time-series
//!   and returns every anomaly the configured detector fires. Fully offline,
//!   deterministic, no data source — the work is done by
//!   [`xiaoguai_anomaly::backtest`].
//! - **`/run`** would evaluate a spec against a *live* KPI data source. That
//!   needs an external time-series store, which the single-binary
//!   embedded-SQLite build deliberately does not wire up (DEC-033), so it
//!   returns a `503` with an explanatory message. The CLI renders any 503 from
//!   these endpoints as a friendly "not available in this build" hint.
//!
//! ## Wire contract (pinned by `crates/xiaoguai-cli/src/commands/anomaly.rs`)
//!
//! Request to `/test`:
//! ```json
//! { "spec": <AnomalySpec>, "csv": "...", "ts_col": "ts", "val_col": "value" }
//! ```
//! Response: `{ "anomalies": [ {ts, value, mean, std, score, description} ], "summary": "..." }`.
//!
//! Note the response field names `mean`/`std` — the CLI table reads those keys,
//! whereas the lib's [`Anomaly`] serialises them as `baseline_mean`/`baseline_std`.
//! [`AnomalyRow`] performs that mapping, so the lib type is never serialised
//! straight onto the wire.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use xiaoguai_anomaly::{Anomaly, AnomalySpec, DetectorKind};

use crate::error::ApiError;

// ─── Request / response bodies ──────────────────────────────────────────────

/// Body of `POST /v1/anomaly/test`.
#[derive(Debug, Deserialize)]
pub struct TestBody {
    /// Declarative monitor spec; only `detector` + `cool_off` affect the
    /// back-test, but a complete spec is required (matches the lib type and the
    /// CLI, which posts the user's full spec file).
    pub spec: AnomalySpec,
    /// Raw CSV text with a header row.
    pub csv: String,
    /// Header name of the timestamp column.
    pub ts_col: String,
    /// Header name of the numeric value column.
    pub val_col: String,
}

/// One detected anomaly, in the shape the CLI table renders.
#[derive(Debug, Serialize, PartialEq)]
pub struct AnomalyRow {
    /// RFC 3339 timestamp of the anomalous observation.
    pub ts: String,
    /// The observed value.
    pub value: f64,
    /// Baseline mean at detection time (`Anomaly::baseline_mean`).
    pub mean: f64,
    /// Baseline std-dev at detection time (`Anomaly::baseline_std`).
    pub std: f64,
    /// Signed deviation score.
    pub score: f64,
    /// Human-readable description.
    pub description: String,
}

impl From<&Anomaly> for AnomalyRow {
    fn from(a: &Anomaly) -> Self {
        Self {
            ts: a.ts.to_rfc3339(),
            value: a.value,
            mean: a.baseline_mean,
            std: a.baseline_std,
            score: a.score,
            description: a.description.clone(),
        }
    }
}

/// Body of a successful `POST /v1/anomaly/test`.
#[derive(Debug, Serialize)]
pub struct TestResponse {
    /// Anomalies in observation order.
    pub anomalies: Vec<AnomalyRow>,
    /// One-line human summary.
    pub summary: String,
}

// ─── CSV parsing (pure, validated at the boundary) ──────────────────────────

/// Parse a single timestamp cell: RFC 3339 first, then integer epoch seconds.
fn parse_ts(raw: &str) -> Result<DateTime<Utc>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(epoch) = raw.parse::<i64>() {
        return match Utc.timestamp_opt(epoch, 0).single() {
            Some(dt) => Ok(dt),
            None => Err(format!("epoch seconds out of range: '{raw}'")),
        };
    }
    Err(format!(
        "unparseable timestamp '{raw}' (expected RFC3339 or integer epoch seconds)"
    ))
}

/// Parse `csv` into `(timestamp, value)` pairs, selecting the `ts_col` /
/// `val_col` columns by header name.
///
/// Minimal comma-split parser (no quoted-field support) — sufficient for
/// numeric time-series where cells are timestamps and floats. Blank lines are
/// skipped. Every failure carries the offending line for a clear 400.
fn parse_csv(csv: &str, ts_col: &str, val_col: &str) -> Result<Vec<(DateTime<Utc>, f64)>, String> {
    let mut lines = csv
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());

    let (_, header_line) = lines.next().ok_or("CSV is empty (no header row)")?;
    let header: Vec<&str> = header_line.split(',').map(str::trim).collect();
    let ts_idx = header
        .iter()
        .position(|h| *h == ts_col)
        .ok_or_else(|| format!("timestamp column '{ts_col}' not found in header"))?;
    let val_idx = header
        .iter()
        .position(|h| *h == val_col)
        .ok_or_else(|| format!("value column '{val_col}' not found in header"))?;
    let need = ts_idx.max(val_idx) + 1;

    let mut points = Vec::new();
    for (line_no, line) in lines {
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        if cells.len() < need {
            return Err(format!(
                "line {}: expected at least {need} columns, found {}",
                line_no + 1,
                cells.len()
            ));
        }
        let ts = parse_ts(cells[ts_idx]).map_err(|e| format!("line {}: {e}", line_no + 1))?;
        let value: f64 = cells[val_idx].parse().map_err(|_| {
            format!(
                "line {}: unparseable value '{}'",
                line_no + 1,
                cells[val_idx]
            )
        })?;
        if !value.is_finite() {
            return Err(format!(
                "line {}: value '{}' is not finite",
                line_no + 1,
                cells[val_idx]
            ));
        }
        points.push((ts, value));
    }
    Ok(points)
}

fn detector_label(kind: &DetectorKind) -> &'static str {
    match kind {
        DetectorKind::ZScore { .. } => "zscore",
        DetectorKind::Ewma { .. } => "ewma",
    }
}

fn summarize(spec: &AnomalySpec, points: usize, anomalies: usize) -> String {
    format!(
        "{anomalies} anomalies in {points} points (detector: {})",
        detector_label(&spec.detector)
    )
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /v1/anomaly/test` — back-test a spec against inline CSV data.
pub async fn backtest(Json(body): Json<TestBody>) -> Response {
    let points = match parse_csv(&body.csv, &body.ts_col, &body.val_col) {
        Ok(p) => p,
        Err(e) => return ApiError::BadRequest(format!("CSV parse error: {e}")).into_response(),
    };
    let anomalies = match xiaoguai_anomaly::backtest(&body.spec, &points) {
        Ok(a) => a,
        Err(e) => return ApiError::BadRequest(e.to_string()).into_response(),
    };
    let rows: Vec<AnomalyRow> = anomalies.iter().map(AnomalyRow::from).collect();
    let summary = summarize(&body.spec, points.len(), rows.len());
    (
        StatusCode::OK,
        Json(TestResponse {
            anomalies: rows,
            summary,
        }),
    )
        .into_response()
}

/// `POST /v1/anomaly/run` — live KPI evaluation. Not wired in the single-binary
/// build (no external data source under DEC-033); always `503`. The body is
/// ignored so the stub is unconditional.
pub async fn run() -> Response {
    ApiError::ServiceUnavailable(
        "live KPI evaluation (anomaly run) needs an external time-series data source, \
         which the single-binary embedded-SQLite build does not wire up (DEC-033). \
         Use POST /v1/anomaly/test to back-test a spec against CSV data."
            .to_string(),
    )
    .into_response()
}

// ─── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ts_rfc3339() {
        let dt = parse_ts("2026-06-21T10:30:00Z").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-06-21T10:30:00+00:00");
    }

    #[test]
    fn parse_ts_epoch_seconds() {
        let dt = parse_ts("0").unwrap();
        assert_eq!(dt.timestamp(), 0);
        let dt = parse_ts("1718965800").unwrap();
        assert_eq!(dt.timestamp(), 1_718_965_800);
    }

    #[test]
    fn parse_ts_rejects_garbage() {
        assert!(parse_ts("not-a-time").is_err());
    }

    #[test]
    fn parse_csv_happy_path_epoch() {
        let csv = "ts,value\n0,100\n1,101\n2,102\n";
        let points = parse_csv(csv, "ts", "value").unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].0.timestamp(), 0);
        assert!((points[2].1 - 102.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_csv_column_order_independent() {
        // value before ts, plus an unrelated column.
        let csv = "value,extra,ts\n100,x,1718965800\n";
        let points = parse_csv(csv, "ts", "value").unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].0.timestamp(), 1_718_965_800);
        assert!((points[0].1 - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_csv_skips_blank_lines() {
        let csv = "ts,value\n\n0,100\n\n1,101\n";
        let points = parse_csv(csv, "ts", "value").unwrap();
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn parse_csv_missing_value_column_errors() {
        let csv = "ts,other\n0,100\n";
        let err = parse_csv(csv, "ts", "value").unwrap_err();
        assert!(err.contains("value column 'value' not found"), "{err}");
    }

    #[test]
    fn parse_csv_missing_ts_column_errors() {
        let csv = "time,value\n0,100\n";
        let err = parse_csv(csv, "ts", "value").unwrap_err();
        assert!(err.contains("timestamp column 'ts' not found"), "{err}");
    }

    #[test]
    fn parse_csv_short_row_errors_with_line_number() {
        let csv = "ts,value\n0,100\n1\n";
        let err = parse_csv(csv, "ts", "value").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert!(err.contains("expected at least 2 columns"), "{err}");
    }

    #[test]
    fn parse_csv_bad_value_errors_with_line_number() {
        let csv = "ts,value\n0,100\n1,abc\n";
        let err = parse_csv(csv, "ts", "value").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert!(err.contains("unparseable value 'abc'"), "{err}");
    }

    #[test]
    fn parse_csv_rejects_non_finite_value() {
        let csv = "ts,value\n0,100\n1,inf\n";
        let err = parse_csv(csv, "ts", "value").unwrap_err();
        assert!(err.contains("not finite"), "{err}");
    }

    #[test]
    fn parse_csv_empty_input_errors() {
        assert!(parse_csv("", "ts", "value").is_err());
        assert!(parse_csv("   \n \n", "ts", "value").is_err());
    }

    #[test]
    fn anomaly_row_maps_baseline_fields() {
        let a = Anomaly {
            ts: Utc.timestamp_opt(42, 0).unwrap(),
            value: 5000.0,
            baseline_mean: 100.0,
            baseline_std: 3.0,
            score: 1633.0,
            description: "spike".to_string(),
        };
        let row = AnomalyRow::from(&a);
        assert_eq!(row.mean, 100.0); // baseline_mean → mean
        assert_eq!(row.std, 3.0); // baseline_std → std
        assert_eq!(row.ts, "1970-01-01T00:00:42+00:00");
    }

    #[test]
    fn summarize_reports_counts_and_detector() {
        let spec = AnomalySpec {
            id: "x".into(),
            kpi_query: "n/a".into(),
            window: chrono::Duration::hours(1),
            detector: DetectorKind::default(),
            cool_off: chrono::Duration::seconds(0),
            on_anomaly: xiaoguai_anomaly::ActionRef::Notify {
                channel: "ops".into(),
            },
        };
        assert_eq!(
            summarize(&spec, 100, 3),
            "3 anomalies in 100 points (detector: zscore)"
        );
    }
}
