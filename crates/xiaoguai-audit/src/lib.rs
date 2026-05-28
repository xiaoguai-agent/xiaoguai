//! Append-only audit log with hmac chain.
//!
//! Every audit entry's hmac is computed over `prev_hmac || canonical_bytes(entry)`,
//! so any tampering breaks the chain. Verified by `verify_chain()`.
//! See ADR-0008 and ADR-0009 for the broader trust + cost-attribution model.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod chain;
pub mod outcomes;
pub mod redact;

pub use chain::{AuditEntry, ChainError, ChainedAudit, StoredEntry};
pub use outcomes::{
    timeseries, Aggregate, InMemoryOutcomeRecorder, OutcomeDay, OutcomeError, OutcomeKind,
    OutcomeRange, OutcomeRecord, OutcomeRecorder, OutcomeSummary,
};
pub use redact::Redactor;
// Re-exported from the leaf crate so `xiaoguai_audit::redact_str` stays stable.
pub use xiaoguai_types::redact_str;
