//! `HotL` argument redaction (DEC-HLD-014).
//!
//! Under the single-user `SQLite` pivot (DEC-033) this crate no longer
//! carries OIDC JWT validation or Casbin RBAC — the API collapses to a
//! single static owner identity gated by an optional username/password.
//! What survives is the store-agnostic [`RedactionRules`] engine used to
//! scrub sensitive `HotL` escalation arguments before they are surfaced or
//! audited.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod redaction;

pub use redaction::{AuthError, RedactionRules};
