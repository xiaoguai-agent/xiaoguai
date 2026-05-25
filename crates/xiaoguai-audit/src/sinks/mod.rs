//! Export sinks for the audit log.
//!
//! Currently ships one sink: [`s3::S3Sink`], which periodically batches new
//! audit rows from Postgres into gzipped NDJSON objects and uploads them to
//! any S3-compatible storage (AWS S3 or `MinIO` via `endpoint_url`).

pub mod s3;
