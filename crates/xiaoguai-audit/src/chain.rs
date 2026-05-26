//! HMAC-SHA256 chained audit log primitives.
//!
//! Every entry's HMAC is computed over `prev_hmac || canonical_bytes(entry)`,
//! producing a tamper-evident append-only log. Any flipped byte, reordered
//! entry, or skipped link breaks [`ChainedAudit::verify_chain`].
//!
//! Canonical encoding is deliberately byte-level (NOT plain `serde_json`)
//! because JSON object key order is unspecified. See module docs for details.

// Persistence sink lives in a sibling file (`src/sink.rs`); we re-host it as a
// submodule here because `lib.rs` is owned by the workspace bootstrap script
// and only declares `pub mod chain;`.
#[path = "sink.rs"]
pub mod sink;

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

/// Length in bytes of an HMAC-SHA256 digest.
pub const HMAC_LEN: usize = 32;

type HmacSha256 = Hmac<Sha256>;

/// Errors that can occur when computing or verifying the audit chain.
#[derive(Debug, Error)]
pub enum ChainError {
    /// The computed HMAC for the row at `id` did not match the stored value.
    #[error("hmac mismatch at row id {0}")]
    HmacMismatch(i64),

    /// Two adjacent entries do not link (entry `b`'s `prev_hmac` != entry `a`'s `hmac`).
    #[error("chain broken between {0} and {1}")]
    LinkBroken(i64, i64),

    /// Canonical encoding of an entry failed.
    #[error("canonical encoding failed: {0}")]
    Canonical(String),

    /// Underlying database error.
    #[error("db: {0}")]
    Database(#[from] sqlx::Error),

    /// A `prev_hmac` or `hmac` had a length other than [`HMAC_LEN`].
    #[error("invalid hmac length")]
    InvalidHmacLength,
}

/// A logical audit event, unsigned. Pair with [`StoredEntry`] for the persisted form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    /// Wall-clock timestamp (UTC). Serialized at nanosecond resolution in canonical bytes.
    pub ts: DateTime<Utc>,
    /// Tenant scope. The chain is per-tenant.
    pub tenant_id: String,
    /// Actor identifier — `user:<id>`, `"system"`, `mcp:<server>`, etc.
    pub actor: String,
    /// Action name — e.g. `session.create`, `tool.invoke`, `cost.charge`.
    pub action: String,
    /// Optional resource identifier (URI / id / path).
    pub resource: Option<String>,
    /// Structured event payload. Key order is normalized in canonical encoding.
    pub details: serde_json::Value,
}

/// A persisted audit entry, with its assigned row id and chain links.
#[derive(Debug, Clone)]
pub struct StoredEntry {
    /// Database row id.
    pub id: i64,
    /// The logical entry that was signed.
    pub entry: AuditEntry,
    /// Previous row's HMAC (or 32 zero bytes if genesis).
    pub prev_hmac: Vec<u8>,
    /// HMAC for this row.
    pub hmac: Vec<u8>,
}

/// Stateless HMAC chain engine. Cheap to clone; holds only the signing key.
#[derive(Clone)]
pub struct ChainedAudit {
    key: Vec<u8>,
}

impl std::fmt::Debug for ChainedAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainedAudit")
            .field("key_len", &self.key.len())
            .finish()
    }
}

impl ChainedAudit {
    /// Create a new chain engine from an HMAC key. Any non-empty byte slice is accepted.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }

    /// Compute the HMAC for `entry` given `prev_hmac`.
    ///
    /// `prev_hmac` MUST be [`HMAC_LEN`] bytes — use a zero-filled buffer for the genesis row.
    pub fn compute_hmac(
        &self,
        prev_hmac: &[u8],
        entry: &AuditEntry,
    ) -> Result<Vec<u8>, ChainError> {
        if prev_hmac.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }
        let canonical = canonical_bytes(entry)?;
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .map_err(|e| ChainError::Canonical(format!("hmac key init: {e}")))?;
        mac.update(prev_hmac);
        mac.update(&canonical);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Verify a chronologically-ordered slice of stored entries forms a valid chain.
    ///
    /// `start_prev` is the `prev_hmac` expected for `entries[0]` — pass `[0u8; 32]`
    /// when verifying from the genesis row.
    pub fn verify_chain(
        &self,
        start_prev: &[u8],
        entries: &[StoredEntry],
    ) -> Result<(), ChainError> {
        if start_prev.len() != HMAC_LEN {
            return Err(ChainError::InvalidHmacLength);
        }
        let mut expected_prev: Vec<u8> = start_prev.to_vec();
        let mut prev_id: Option<i64> = None;
        for stored in entries {
            if stored.prev_hmac.len() != HMAC_LEN || stored.hmac.len() != HMAC_LEN {
                return Err(ChainError::InvalidHmacLength);
            }
            if stored.prev_hmac != expected_prev {
                let from = prev_id.unwrap_or(0);
                return Err(ChainError::LinkBroken(from, stored.id));
            }
            let computed = self.compute_hmac(&stored.prev_hmac, &stored.entry)?;
            if computed != stored.hmac {
                return Err(ChainError::HmacMismatch(stored.id));
            }
            expected_prev.clone_from(&stored.hmac);
            prev_id = Some(stored.id);
        }
        Ok(())
    }
}

/// Produce deterministic canonical bytes for an [`AuditEntry`].
///
/// Format:
/// ```text
/// ts_rfc3339_utc_z || 0x00 ||
/// tenant_id        || 0x00 ||
/// actor            || 0x00 ||
/// action           || 0x00 ||
/// resource_or_empty|| 0x00 ||
/// details_sorted_json_bytes
/// ```
fn canonical_bytes(entry: &AuditEntry) -> Result<Vec<u8>, ChainError> {
    let ts_str = entry.ts.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let details_canonical = canonical_json_value(&entry.details);
    let details_bytes = serde_json::to_vec(&details_canonical)
        .map_err(|e| ChainError::Canonical(format!("details json: {e}")))?;

    let mut buf = Vec::with_capacity(
        ts_str.len()
            + entry.tenant_id.len()
            + entry.actor.len()
            + entry.action.len()
            + entry.resource.as_ref().map_or(0, String::len)
            + details_bytes.len()
            + 5,
    );
    buf.extend_from_slice(ts_str.as_bytes());
    buf.push(0);
    buf.extend_from_slice(entry.tenant_id.as_bytes());
    buf.push(0);
    buf.extend_from_slice(entry.actor.as_bytes());
    buf.push(0);
    buf.extend_from_slice(entry.action.as_bytes());
    buf.push(0);
    if let Some(r) = entry.resource.as_ref() {
        buf.extend_from_slice(r.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&details_bytes);
    Ok(buf)
}

/// Recursively rebuild a `serde_json::Value` with all object keys sorted lex.
///
/// `serde_json::Map` preserves insertion order; we coerce through `BTreeMap` to
/// guarantee a single canonical key order regardless of how the value was built.
fn canonical_json_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, serde_json::Value> = map
                .iter()
                .map(|(k, val)| (k, canonical_json_value(val)))
                .collect();
            let rebuilt: serde_json::Map<String, serde_json::Value> =
                sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect();
            serde_json::Value::Object(rebuilt)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonical_json_value).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod inline_tests {
    use super::{canonical_json_value, AuditEntry, ChainError, ChainedAudit};
    use chrono::{DateTime, Utc};
    use serde_json::json;

    fn sample_entry() -> AuditEntry {
        AuditEntry {
            ts: DateTime::parse_from_rfc3339("2026-05-20T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            tenant_id: "t1".into(),
            actor: "user:1".into(),
            action: "session.create".into(),
            resource: Some("session:abc".into()),
            details: json!({ "b": 2, "a": 1 }),
        }
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let a = json!({ "a": 1, "b": 2 });
        let b = json!({ "b": 2, "a": 1 });
        assert_eq!(
            serde_json::to_string(&canonical_json_value(&a)).unwrap(),
            serde_json::to_string(&canonical_json_value(&b)).unwrap()
        );
    }

    #[test]
    fn compute_hmac_rejects_bad_prev_length() {
        let chain = ChainedAudit::new(b"k".to_vec());
        let bad_prev = vec![0u8; 16];
        let e = chain.compute_hmac(&bad_prev, &sample_entry()).unwrap_err();
        assert!(matches!(e, ChainError::InvalidHmacLength));
    }
}
