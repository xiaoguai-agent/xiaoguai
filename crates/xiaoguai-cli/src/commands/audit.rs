//! `xiaoguai audit` subcommand — audit log management and S3 export.
//!
//! # CLI surface
//!
//! ```text
//! xiaoguai audit export --sink s3 --once
//! xiaoguai audit export --sink s3          # daemon mode (loops)
//! ```
//!
//! The `--once` flag runs a single export cycle and exits with a non-zero
//! code if no rows were exported (useful in cron / CI).  Without `--once`
//! the command runs the export loop using the interval from config.

use anyhow::{Context, Result};
use std::time::Duration;
use xiaoguai_audit::sinks::s3::{S3Sink, S3SinkConfig};

/// Arguments for `xiaoguai audit export`.
#[derive(Debug)]
pub struct ExportArgs {
    /// Sink type — currently only `"s3"` is supported.
    pub sink: String,

    /// S3 bucket name.
    pub bucket: String,

    /// S3 key prefix.
    pub prefix: String,

    /// AWS region.
    pub region: String,

    /// Optional endpoint URL for `MinIO` / Ceph / localstack.
    pub endpoint_url: Option<String>,

    /// Logical sink name (used as primary key in `audit_export_state`).
    pub sink_name: String,

    /// Export interval in seconds (daemon mode).  Ignored with `--once`.
    pub interval_secs: u64,

    /// If `true`, run one cycle then exit.
    pub once: bool,

    /// Postgres connection URL.
    pub database_url: String,
}

/// Run the audit export command.
///
/// Returns `Ok(rows_exported)` for `--once` mode.  In daemon mode this
/// function never returns under normal operation.
///
/// # Errors
/// Returns an error if the database connection or S3 upload fails.
pub async fn run_export(args: ExportArgs) -> Result<usize> {
    anyhow::ensure!(
        args.sink == "s3",
        "unsupported sink '{}'; only 's3' is implemented",
        args.sink
    );

    let pool = sqlx::PgPool::connect(&args.database_url)
        .await
        .context("connect to postgres for audit export")?;

    let mut cfg = S3SinkConfig::new(&args.bucket, &args.prefix, &args.region);
    cfg.endpoint_url = args.endpoint_url;
    cfg.sink_name = args.sink_name;
    cfg.interval = Duration::from_secs(args.interval_secs);

    let sink = S3Sink::new(pool, cfg).await;

    if args.once {
        let n = sink.run_once().await.context("audit export cycle")?;
        Ok(n)
    } else {
        sink.run_loop().await;
        Ok(0) // unreachable in practice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_args_s3_sink_name_set() {
        let args = ExportArgs {
            sink: "s3".into(),
            bucket: "my-bucket".into(),
            prefix: "audit".into(),
            region: "us-east-1".into(),
            endpoint_url: None,
            sink_name: "prod".into(),
            interval_secs: 3600,
            once: true,
            database_url: "postgres://localhost/test".into(),
        };
        assert_eq!(args.sink_name, "prod");
        assert!(args.once);
    }

    #[tokio::test]
    async fn export_rejects_unknown_sink() {
        let args = ExportArgs {
            sink: "kafka".into(),
            bucket: "b".into(),
            prefix: "p".into(),
            region: "us-east-1".into(),
            endpoint_url: None,
            sink_name: "default".into(),
            interval_secs: 3600,
            once: true,
            database_url: "postgres://localhost/test".into(),
        };
        let err = run_export(args).await.unwrap_err();
        assert!(err.to_string().contains("unsupported sink"));
    }
}
