//! Append-only audit log with hmac chain.
//!
//! Every audit entry's hmac is computed over `prev_hmac || canonical_bytes(entry)`,
//! so any tampering breaks the chain. Verified by `verify_chain()`.
//! See ADR-0008 and ADR-0009 for the broader trust + cost-attribution model.
//!
//! # S3 export
//!
//! [`sinks::s3::S3Sink`] periodically exports new audit rows to any
//! S3-compatible store (AWS S3 or `MinIO`). See that module for details.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod chain;
pub mod sinks;

pub use chain::{AuditEntry, ChainError, ChainedAudit, StoredEntry};
