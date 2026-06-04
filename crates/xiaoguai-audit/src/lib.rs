//! Append-only audit log with hmac chain.
//!
//! Every audit entry's hmac is computed over `prev_hmac || canonical_bytes(entry)`,
//! so any tampering breaks the chain. Verified by `verify_chain()`.
//! See ADR-0008 and ADR-0009 for the broader trust + cost-attribution model.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

pub mod chain;
pub mod export;
pub mod outcomes;
pub mod pdf;
pub mod redact;

/// Single-user owner identity signed into every audit HMAC entry's `tenant_id`.
/// The `tenant_id` column was dropped in the SQLite pivot (DEC-033); the chain
/// still hashes a `tenant_id`, so **every append site and the read-side rebuild
/// must use this exact value** or `verify_chain` reports a broken chain.
pub const OWNER_TENANT_ID: &str = "ten_local_owner";

pub use chain::{AuditEntry, ChainError, ChainedAudit, StoredEntry};
pub use export::{
    export_bundle, render, render_csv, render_json, render_pdf, BundleHeader, BundleRow,
    ChainProof, ComplianceBundle, ExportError, ExportWindow, Format, Framework,
};
pub use outcomes::{
    timeseries, Aggregate, InMemoryOutcomeRecorder, OutcomeDay, OutcomeError, OutcomeKind,
    OutcomeRange, OutcomeRecord, OutcomeRecorder, OutcomeSummary,
};
pub use redact::Redactor;
// Re-exported from the leaf crate so `xiaoguai_audit::redact_str` stays stable.
pub use xiaoguai_types::redact_str;
