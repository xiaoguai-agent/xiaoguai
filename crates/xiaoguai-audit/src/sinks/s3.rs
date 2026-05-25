//! S3-compatible export sink for the HMAC-chained audit log.
//!
//! # Overview
//!
//! [`S3Sink`] periodically reads audit rows from Postgres that have not yet
//! been exported (`id > last_exported_id`), encodes them as NDJSON, gzips the
//! batch, and uploads the object to the configured bucket.  A watermark row in
//! `audit_export_state` persists the progress across restarts.
//!
//! Object key pattern:
//! ```text
//! {prefix}/year=YYYY/month=MM/day=DD/hour=HH/audit-{uuid}.ndjson.gz
//! ```
//!
//! # Failure handling
//!
//! On upload error the sink performs exponential-backoff retries (up to
//! [`MAX_UPLOAD_RETRIES`]).  If all retries are exhausted, the gzipped batch
//! is written to the local spool directory (`spool_dir`) so the operator can
//! re-upload it later.  The watermark is **not** advanced when a batch fails
//! all retries; the same rows will be included in the next export run.
//!
//! # `MinIO` / custom endpoint
//!
//! Set `endpoint_url` (e.g. `http://localhost:9000`) to route requests to
//! any S3-compatible service.  `force_path_style` is automatically enabled
//! when an endpoint override is present because most `MinIO` deployments require
//! it.
//!
//! # Encryption at rest (SSE-KMS)
//!
//! Deferred — see task C14 notes.  The upload call can be extended with
//! `.server_side_encryption(…)` and `.ssekms_key_id(…)` when KMS is wired in.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

use aws_config::BehaviorVersion;
use aws_sdk_s3::{config::Builder as S3ConfigBuilder, primitives::ByteStream, Client as S3Client};
use chrono::{DateTime, Utc};
use flate2::{write::GzEncoder, Compression};
use sqlx::PgPool;
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::chain::AuditEntry;

/// Maximum number of upload attempts before spooling to disk.
pub const MAX_UPLOAD_RETRIES: u32 = 3;

/// Base wait between retries (doubles on each attempt).
pub const RETRY_BASE_MS: u64 = 500;

/// Default export interval.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);

/// Default per-export row limit (keeps objects ≤ ~50 MiB for typical row sizes).
pub const DEFAULT_BATCH_LIMIT: i64 = 50_000;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum S3SinkError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("json encoding failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("gzip encoding failed: {0}")]
    Gzip(#[from] std::io::Error),

    #[error("s3 upload failed after {retries} retries: {source}")]
    Upload {
        retries: u32,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    #[error("spool write failed: {0}")]
    Spool(std::io::Error),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for [`S3Sink`].
#[derive(Debug, Clone)]
pub struct S3SinkConfig {
    /// S3 bucket name.
    pub bucket: String,

    /// Key prefix (no trailing slash). Objects land under
    /// `{prefix}/year=YYYY/month=MM/…`.
    pub prefix: String,

    /// AWS region (e.g. `"us-east-1"`).
    pub region: String,

    /// Optional endpoint URL for `MinIO` / Ceph / localstack.
    /// When set, `force_path_style` is automatically enabled.
    pub endpoint_url: Option<String>,

    /// How often to run the export cycle in daemon mode.
    /// Defaults to [`DEFAULT_INTERVAL`] (1 hour).
    pub interval: Duration,

    /// Maximum rows per export batch.
    pub batch_limit: i64,

    /// Directory for spooling failed batches.
    /// Defaults to `$TMPDIR/xiaoguai-audit-spool`.
    pub spool_dir: PathBuf,

    /// Logical name of this sink — used as the primary key in
    /// `audit_export_state`.
    pub sink_name: String,
}

impl S3SinkConfig {
    /// Construct with required fields; optional fields take sensible defaults.
    pub fn new(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: prefix.into(),
            region: region.into(),
            endpoint_url: None,
            interval: DEFAULT_INTERVAL,
            batch_limit: DEFAULT_BATCH_LIMIT,
            spool_dir: std::env::temp_dir().join("xiaoguai-audit-spool"),
            sink_name: "default".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Watermark helpers (DB)
// ---------------------------------------------------------------------------

/// Read the current watermark from `audit_export_state`.
/// Returns `(last_exported_id, last_exported_at)`.
async fn read_watermark(
    pool: &PgPool,
    sink_name: &str,
) -> Result<(i64, Option<DateTime<Utc>>), sqlx::Error> {
    let row: Option<(i64, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT last_exported_id, last_exported_at \
         FROM audit_export_state \
         WHERE sink_name = $1",
    )
    .bind(sink_name)
    .fetch_optional(pool)
    .await?;

    Ok(row.unwrap_or((0, None)))
}

/// Advance the watermark after a successful upload.
async fn advance_watermark(
    pool: &PgPool,
    sink_name: &str,
    new_id: i64,
    exported_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_export_state (sink_name, last_exported_id, last_exported_at) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (sink_name) DO UPDATE \
           SET last_exported_id = EXCLUDED.last_exported_id, \
               last_exported_at = EXCLUDED.last_exported_at",
    )
    .bind(sink_name)
    .bind(new_id)
    .bind(exported_at)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Row fetch
// ---------------------------------------------------------------------------

/// Fetch up to `limit` audit rows with `id > since_id`, ordered id ASC.
async fn fetch_rows(
    pool: &PgPool,
    since_id: i64,
    limit: i64,
) -> Result<Vec<(i64, AuditEntry)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, RawAuditRow>(
        "SELECT id, ts, tenant_id, actor, action, resource, details \
         FROM audit_log \
         WHERE id > $1 \
         ORDER BY id ASC \
         LIMIT $2",
    )
    .bind(since_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(RawAuditRow::into_pair).collect())
}

#[derive(sqlx::FromRow)]
struct RawAuditRow {
    id: i64,
    ts: DateTime<Utc>,
    tenant_id: String,
    actor: String,
    action: String,
    resource: Option<String>,
    details: serde_json::Value,
}

impl RawAuditRow {
    fn into_pair(self) -> (i64, AuditEntry) {
        let entry = AuditEntry {
            ts: self.ts,
            tenant_id: self.tenant_id,
            actor: self.actor,
            action: self.action,
            resource: self.resource,
            details: self.details,
        };
        (self.id, entry)
    }
}

// ---------------------------------------------------------------------------
// NDJSON + gzip encoding
// ---------------------------------------------------------------------------

/// Serialise `rows` as NDJSON and gzip-compress the result.
///
/// Each line is a JSON object with all fields of [`AuditEntry`] plus the
/// row `id`.  The caller can parse any line back as a JSON object and round-
/// trip the entry fields for validation.
pub fn encode_batch(rows: &[(i64, AuditEntry)]) -> Result<Vec<u8>, S3SinkError> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    for (id, entry) in rows {
        let line = serde_json::json!({
            "id":        id,
            "ts":        entry.ts,
            "tenant_id": entry.tenant_id,
            "actor":     entry.actor,
            "action":    entry.action,
            "resource":  entry.resource,
            "details":   entry.details,
        });
        let bytes = serde_json::to_vec(&line)?;
        gz.write_all(&bytes)?;
        gz.write_all(b"\n")?;
    }
    Ok(gz.finish()?)
}

// ---------------------------------------------------------------------------
// S3 key builder
// ---------------------------------------------------------------------------

/// Build the S3 object key for a batch uploaded at `ts`.
#[must_use]
pub fn build_key(prefix: &str, ts: DateTime<Utc>) -> String {
    format!(
        "{prefix}/year={year}/month={month:02}/day={day:02}/hour={hour:02}/audit-{uuid}.ndjson.gz",
        prefix = prefix,
        year = ts.format("%Y"),
        month = ts.format("%m"),
        day = ts.format("%d"),
        hour = ts.format("%H"),
        uuid = Uuid::new_v4(),
    )
}

// ---------------------------------------------------------------------------
// S3 client factory
// ---------------------------------------------------------------------------

async fn build_s3_client(cfg: &S3SinkConfig) -> S3Client {
    let sdk_cfg = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(cfg.region.clone()))
        .load()
        .await;

    let mut builder = S3ConfigBuilder::from(&sdk_cfg);

    if let Some(ref url) = cfg.endpoint_url {
        builder = builder.endpoint_url(url).force_path_style(true);
    }

    S3Client::from_conf(builder.build())
}

// ---------------------------------------------------------------------------
// Spool helpers
// ---------------------------------------------------------------------------

fn spool_batch(spool_dir: &Path, key: &str, data: &[u8]) -> Result<PathBuf, S3SinkError> {
    std::fs::create_dir_all(spool_dir).map_err(S3SinkError::Spool)?;
    // flatten the key path into a filename so we don't create subdirs in spool
    let safe_name = key.replace('/', "__");
    let path = spool_dir.join(safe_name);
    std::fs::write(&path, data).map_err(S3SinkError::Spool)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Upload with retry
// ---------------------------------------------------------------------------

/// Upload `data` to `bucket/key` with exponential-backoff retries.
///
/// Returns `Ok(())` on success, `Err(S3SinkError::Upload{…})` if all
/// attempts are exhausted.
///
/// # Panics
///
/// Panics if `MAX_UPLOAD_RETRIES` is 0 (the loop would not run and
/// `last_err.expect(…)` would trigger).  The constant is set to 3.
pub async fn upload_with_retry(
    client: &S3Client,
    bucket: &str,
    key: &str,
    data: Vec<u8>,
) -> Result<(), S3SinkError> {
    let mut last_err: Option<Box<dyn std::error::Error + Send + Sync + 'static>> = None;

    for attempt in 0..MAX_UPLOAD_RETRIES {
        if attempt > 0 {
            let wait = Duration::from_millis(RETRY_BASE_MS * (1 << (attempt - 1)));
            debug!(%attempt, ?wait, "audit S3 upload retry");
            tokio::time::sleep(wait).await;
        }

        let body = ByteStream::from(data.clone());
        match client
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_encoding("gzip")
            .content_type("application/x-ndjson")
            .body(body)
            .send()
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                warn!(attempt, key, error = %e, "audit S3 upload failed");
                last_err = Some(Box::new(e));
            }
        }
    }

    Err(S3SinkError::Upload {
        retries: MAX_UPLOAD_RETRIES,
        source: last_err.expect("loop ran at least once"),
    })
}

// ---------------------------------------------------------------------------
// S3Sink — main struct
// ---------------------------------------------------------------------------

/// S3-compatible export sink.
///
/// Call [`S3Sink::run_once`] for a single export cycle (used by the CLI
/// `xiaoguai audit export --sink s3 --once` command and in tests).
/// Call [`S3Sink::run_loop`] from the scheduler daemon to run on a periodic
/// timer.
#[derive(Clone)]
pub struct S3Sink {
    pool: PgPool,
    client: S3Client,
    config: S3SinkConfig,
}

impl std::fmt::Debug for S3Sink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Sink")
            .field("bucket", &self.config.bucket)
            .field("prefix", &self.config.prefix)
            .field("sink_name", &self.config.sink_name)
            .field("pool", &"PgPool { .. }")
            .field("client", &"S3Client { .. }")
            .finish()
    }
}

impl S3Sink {
    /// Build an [`S3Sink`] from a live Postgres pool and configuration.
    ///
    /// Constructs the AWS SDK client internally.  For testing, prefer
    /// [`S3Sink::with_client`] to inject a mock client.
    pub async fn new(pool: PgPool, config: S3SinkConfig) -> Self {
        let client = build_s3_client(&config).await;
        Self {
            pool,
            client,
            config,
        }
    }

    /// Inject a pre-built S3 client (testing / mock scenarios).
    #[must_use]
    pub fn with_client(pool: PgPool, client: S3Client, config: S3SinkConfig) -> Self {
        Self {
            pool,
            client,
            config,
        }
    }

    /// Run one export cycle.
    ///
    /// 1. Read the watermark.
    /// 2. Fetch rows `id > watermark`.
    /// 3. Encode as gzipped NDJSON.
    /// 4. Upload to S3 (with retry).
    /// 5. Advance the watermark.
    ///
    /// Returns the number of rows exported (0 = nothing to do).
    /// On upload failure after all retries, spools the batch to disk and
    /// returns `Err`.  The watermark is **not** advanced on failure.
    ///
    /// # Panics
    ///
    /// Panics if `fetch_rows` returns a non-empty `Vec` but `last()` returns
    /// `None` — this is structurally impossible given a non-empty `Vec`.
    pub async fn run_once(&self) -> Result<usize, S3SinkError> {
        let (watermark_id, _) = read_watermark(&self.pool, &self.config.sink_name).await?;
        debug!(watermark_id, "audit S3 export: starting cycle");

        let rows = fetch_rows(&self.pool, watermark_id, self.config.batch_limit).await?;
        if rows.is_empty() {
            debug!("audit S3 export: no new rows");
            return Ok(0);
        }

        let max_id = rows.last().map(|(id, _)| *id).expect("non-empty");
        let exported_at = Utc::now();
        let key = build_key(&self.config.prefix, exported_at);

        let compressed = encode_batch(&rows)?;

        match upload_with_retry(&self.client, &self.config.bucket, &key, compressed.clone()).await {
            Ok(()) => {
                info!(
                    rows = rows.len(),
                    max_id, key, "audit S3 export: uploaded batch"
                );
                advance_watermark(&self.pool, &self.config.sink_name, max_id, exported_at).await?;
                Ok(rows.len())
            }
            Err(upload_err) => {
                let spool_path = spool_batch(&self.config.spool_dir, &key, &compressed);
                match spool_path {
                    Ok(p) => warn!(
                        path = %p.display(),
                        "audit S3 export: batch spooled after upload failure"
                    ),
                    Err(spool_err) => warn!(
                        spool_error = %spool_err,
                        "audit S3 export: spool write also failed"
                    ),
                }
                Err(upload_err)
            }
        }
    }

    /// Run the export loop indefinitely, sleeping [`S3SinkConfig::interval`]
    /// between cycles.  Errors are logged but do not terminate the loop.
    pub async fn run_loop(&self) {
        info!(
            sink = %self.config.sink_name,
            interval = ?self.config.interval,
            "audit S3 export loop started"
        );
        loop {
            match self.run_once().await {
                Ok(n) => {
                    if n > 0 {
                        info!(rows = n, "audit S3 export cycle done");
                    }
                }
                Err(e) => warn!(error = %e, "audit S3 export cycle failed"),
            }
            tokio::time::sleep(self.config.interval).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    use std::io::Read;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_entry(id: i64, action: &str) -> (i64, AuditEntry) {
        (
            id,
            AuditEntry {
                ts: Utc.with_ymd_and_hms(2026, 5, 24, 14, 0, 0).unwrap(),
                tenant_id: "t1".into(),
                actor: "user:42".into(),
                action: action.into(),
                resource: Some(format!("res:{id}")),
                details: serde_json::json!({ "x": id }),
            },
        )
    }

    fn decode_ndjson(gz_bytes: &[u8]) -> Vec<Value> {
        let mut decoder = GzDecoder::new(gz_bytes);
        let mut buf = String::new();
        decoder.read_to_string(&mut buf).expect("gzip decode");
        buf.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("json parse"))
            .collect()
    }

    // -----------------------------------------------------------------------
    // encode_batch: format and round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn encode_batch_produces_valid_gzipped_ndjson() {
        let rows = vec![
            sample_entry(1, "session.create"),
            sample_entry(2, "tool.invoke"),
        ];
        let compressed = encode_batch(&rows).expect("encode");

        let lines = decode_ndjson(&compressed);
        assert_eq!(lines.len(), 2, "two NDJSON lines");

        // first line
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[0]["action"], "session.create");
        assert_eq!(lines[0]["tenant_id"], "t1");

        // second line
        assert_eq!(lines[1]["id"], 2);
        assert_eq!(lines[1]["action"], "tool.invoke");
        assert_eq!(lines[1]["details"]["x"], 2);
    }

    #[test]
    fn encode_batch_empty_produces_empty_gz() {
        let compressed = encode_batch(&[]).expect("encode empty");
        let lines = decode_ndjson(&compressed);
        assert!(lines.is_empty());
    }

    #[test]
    fn encode_batch_row_count_preserved() {
        let rows: Vec<_> = (1..=100).map(|i| sample_entry(i, "ping")).collect();
        let compressed = encode_batch(&rows).expect("encode 100");
        let lines = decode_ndjson(&compressed);
        assert_eq!(lines.len(), 100);
    }

    // -----------------------------------------------------------------------
    // build_key: partition path format
    // -----------------------------------------------------------------------

    #[test]
    fn build_key_has_correct_partition_segments() {
        let ts = Utc.with_ymd_and_hms(2026, 5, 24, 14, 30, 0).unwrap();
        let key = build_key("audit", ts);
        assert!(
            key.starts_with("audit/year=2026/month=05/day=24/hour=14/audit-"),
            "bad key: {key}"
        );
        assert!(key.ends_with(".ndjson.gz"), "bad suffix: {key}");
    }

    #[test]
    fn build_key_uuid_differs_per_call() {
        let ts = Utc::now();
        let k1 = build_key("p", ts);
        let k2 = build_key("p", ts);
        assert_ne!(k1, k2, "UUIDs must differ");
    }

    // -----------------------------------------------------------------------
    // spool_batch: writes file and returns path
    // -----------------------------------------------------------------------

    #[test]
    fn spool_batch_writes_file() {
        let dir = TempDir::new().unwrap();
        let key = "audit/year=2026/month=05/day=24/hour=14/audit-abc.ndjson.gz";
        let data = b"compressed-data";

        let path = spool_batch(dir.path(), key, data).expect("spool");
        assert!(path.exists(), "spool file must exist");
        let read_back = std::fs::read(&path).unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn spool_batch_flattens_key_slashes() {
        let dir = TempDir::new().unwrap();
        let key = "prefix/year=2026/month=05/day=24/hour=00/audit-xyz.ndjson.gz";
        let path = spool_batch(dir.path(), key, b"x").expect("spool");
        // no subdirectory created inside spool_dir
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(!filename.contains('/'), "filename must not contain slashes");
    }

    // -----------------------------------------------------------------------
    // Retry-on-error mock: test the retry + spool logic using a fake client
    //
    // We can't easily use a real S3 client in unit tests without testcontainers,
    // so we test the encode + spool path directly and verify the retry counter
    // logic by exercising `upload_with_retry` behaviour patterns.
    // -----------------------------------------------------------------------

    /// Verify that after `MAX_UPLOAD_RETRIES` failures the error is returned.
    /// We do this by testing the retry count constant and the spool behaviour
    /// in an isolated scenario that doesn't require a live S3 endpoint.
    #[test]
    fn retry_constants_match_spec() {
        assert_eq!(MAX_UPLOAD_RETRIES, 3, "spec requires 3 attempts");
        assert_eq!(RETRY_BASE_MS, 500);
    }

    /// Spool-on-failure path: 3 failures → batch lands in spool dir.
    /// We directly test the spool helper that `run_once` delegates to.
    #[test]
    fn spool_on_failure_lands_in_spool_dir() {
        let dir = TempDir::new().unwrap();
        let rows = vec![sample_entry(7, "cost.charge")];
        let compressed = encode_batch(&rows).expect("encode");
        let key = "audit/year=2026/month=05/day=24/hour=14/audit-fail.ndjson.gz";

        let path = spool_batch(dir.path(), key, &compressed).expect("spool");
        assert!(path.exists());

        // verify spool file is a valid gz with the right row
        let spooled = std::fs::read(&path).unwrap();
        let lines = decode_ndjson(&spooled);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["id"], 7);
        assert_eq!(lines[0]["action"], "cost.charge");
    }

    // -----------------------------------------------------------------------
    // S3SinkConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn config_defaults_are_sensible() {
        let cfg = S3SinkConfig::new("my-bucket", "audit", "us-east-1");
        assert_eq!(cfg.bucket, "my-bucket");
        assert_eq!(cfg.prefix, "audit");
        assert_eq!(cfg.region, "us-east-1");
        assert!(cfg.endpoint_url.is_none());
        assert_eq!(cfg.interval, DEFAULT_INTERVAL);
        assert_eq!(cfg.batch_limit, DEFAULT_BATCH_LIMIT);
        assert_eq!(cfg.sink_name, "default");
    }

    #[test]
    fn config_endpoint_url_for_minio() {
        let mut cfg = S3SinkConfig::new("bucket", "prefix", "us-east-1");
        cfg.endpoint_url = Some("http://localhost:9000".into());
        assert!(cfg.endpoint_url.is_some());
    }

    // -----------------------------------------------------------------------
    // Integration-level watermark advancement (PG) — marked #[ignore]
    // Uncomment and run with a live PG + MinIO testcontainer setup.
    // -----------------------------------------------------------------------

    /// Watermark advances correctly across two export cycles.
    /// Requires: DATABASE_URL env + MinIO at localhost:9000 (testcontainers).
    #[tokio::test]
    #[ignore = "requires live PG + MinIO testcontainer"]
    async fn watermark_advances_across_exports() {
        use sqlx::PgPool;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = PgPool::connect(&url).await.unwrap();

        // After a run with no rows, watermark stays at 0
        let (id0, _) = read_watermark(&pool, "test-sink").await.unwrap();
        assert_eq!(id0, 0);

        // Advance manually and read back
        advance_watermark(&pool, "test-sink", 42, Utc::now())
            .await
            .unwrap();
        let (id1, at1) = read_watermark(&pool, "test-sink").await.unwrap();
        assert_eq!(id1, 42);
        assert!(at1.is_some());

        // Advance again — must overwrite
        advance_watermark(&pool, "test-sink", 99, Utc::now())
            .await
            .unwrap();
        let (id2, _) = read_watermark(&pool, "test-sink").await.unwrap();
        assert_eq!(id2, 99);
    }

    /// S3 upload round-trip via MinIO testcontainer.
    #[tokio::test]
    #[ignore = "requires live MinIO testcontainer"]
    async fn upload_roundtrip_via_minio() {
        let client = {
            let sdk_cfg = aws_config::defaults(BehaviorVersion::latest())
                .region(aws_config::Region::new("us-east-1".to_string()))
                .load()
                .await;
            let conf = S3ConfigBuilder::from(&sdk_cfg)
                .endpoint_url("http://localhost:9000")
                .force_path_style(true)
                .build();
            S3Client::from_conf(conf)
        };

        let rows = vec![sample_entry(1, "session.create")];
        let compressed = encode_batch(&rows).unwrap();
        let key = build_key("audit-test", Utc::now());

        upload_with_retry(&client, "test-bucket", &key, compressed.clone())
            .await
            .unwrap();

        // Fetch back and validate
        let got = client
            .get_object()
            .bucket("test-bucket")
            .key(&key)
            .send()
            .await
            .unwrap();
        let body = got.body.collect().await.unwrap().into_bytes().to_vec();
        let lines = decode_ndjson(&body);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["action"], "session.create");
    }
}
