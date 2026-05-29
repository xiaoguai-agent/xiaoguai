//! v0.6.4 — audit log read surface for the admin endpoint.
//!
//! `GET /v1/admin/audit` lists tamper-evident audit rows for a tenant.
//! To keep `xiaoguai-api` decoupled from the concrete persistence layer
//! (`PgAuditSink` lives in `xiaoguai-audit`, which is *not* an api dep
//! today), we define an `AuditReader` trait here and ship a thin bridge
//! that wraps any `xiaoguai-audit::sink::PgAuditSink` once at boot time.
//!
//! Wire shape (`AuditEntryView`) is the JSON-friendly projection — `prev_hmac`
//! and `hmac` are hex-encoded, `details` passes through as-is.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum AuditError {
    #[error("audit backend: {0}")]
    Backend(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// JSON-friendly audit row served by `GET /v1/admin/audit`.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntryView {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub tenant_id: String,
    pub actor: String,
    pub action: String,
    pub resource: Option<String>,
    pub details: serde_json::Value,
    /// Lowercase hex.
    pub prev_hmac: String,
    /// Lowercase hex.
    pub hmac: String,
}

#[async_trait]
pub trait AuditReader: Send + Sync {
    async fn list(
        &self,
        tenant_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<AuditEntryView>, AuditError>;
}

/// v0.6.5 — chain-integrity verifier surfaced via
/// `GET /v1/admin/audit/verify`. Reports the row id at which the chain
/// breaks (`Err(VerifyReport::Broken { row_id })`) or the count of
/// verified rows on success. Production wires the
/// `xiaoguai-audit::PgAuditSink` implementation.
#[async_trait]
pub trait AuditVerifier: Send + Sync {
    async fn verify_tenant(&self, tenant_id: &str) -> Result<VerifyReport, AuditError>;
}

/// Outcome of a chain-integrity walk for one tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyReport {
    /// All rows verified; `verified_count` rows were walked.
    Ok { verified_count: u64 },
    /// Chain broke at the given row id. The endpoint returns 200 with
    /// `{"ok": false, "broken_at": rowid}` so monitoring can scrape it.
    Broken { broken_at: i64 },
}

/// v1.5 (T5) — compliance bundle exporter.
///
/// Wraps `xiaoguai-audit::export::export_bundle` behind a trait so the api
/// crate doesn't depend on `xiaoguai-audit` directly. The Pg adapter in
/// `xiaoguai-core::audit_bridge` reads rows for `[from, to]`, calls
/// `export_bundle` (which re-verifies chain continuity inside the window),
/// then renders to the requested format and returns the raw bytes.
///
/// Why a separate trait (vs. reusing `AuditReader`): the export requires the
/// signing key (the `ChainedAudit` engine), and the api crate must never see
/// the key. Keeping the work inside the bridge preserves that boundary.
#[async_trait]
pub trait AuditChainExporter: Send + Sync {
    /// Build + render a compliance bundle for `[from, to]`.
    ///
    /// `framework` and `format` are short strings (e.g. `"soc2"`, `"json"`)
    /// parsed inside the adapter. Returns the rendered bytes (`Content-Type`
    /// is the caller's job) on success.
    async fn export(&self, req: ExportRequest) -> Result<Vec<u8>, ExportError>;
}

/// Request shape for an audit-chain export call.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub tenant_id: String,
    /// Short framework name — `"soc2"`, `"gdpr"`, `"hipaa"`.
    pub framework: String,
    /// Short format name — `"json"`, `"csv"`, `"pdf"`.
    pub format: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// Errors surfaced by [`AuditChainExporter::export`].
///
/// Modelled to map cleanly onto HTTP status codes in the route handler:
/// * `ChainBroken` → 409
/// * `PdfUnimplemented` → 501
/// * `InvalidArgument` → 400
/// * `Backend` → 500
#[derive(Debug, Clone, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ExportError {
    #[error("audit chain broken at row {first_broken_id} ({first_broken_ts})")]
    ChainBroken {
        first_broken_id: i64,
        first_broken_ts: DateTime<Utc>,
    },

    #[error("pdf rendering is not yet implemented")]
    PdfUnimplemented,

    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },

    #[error("backend: {message}")]
    Backend { message: String },
}

/// In-memory `AuditReader` used by route tests. Holds a fixed list and
/// filters on read.
#[derive(Debug, Default)]
pub struct StaticAuditReader {
    pub rows: Vec<AuditEntryView>,
}

impl StaticAuditReader {
    #[must_use]
    pub fn with_rows(rows: Vec<AuditEntryView>) -> Self {
        Self { rows }
    }
}

#[async_trait]
impl AuditReader for StaticAuditReader {
    async fn list(
        &self,
        tenant_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<AuditEntryView>, AuditError> {
        if limit < 0 {
            return Err(AuditError::InvalidArgument("limit must be >= 0".into()));
        }
        let take = usize::try_from(limit).unwrap_or(usize::MAX);
        Ok(self
            .rows
            .iter()
            .filter(|r| r.tenant_id == tenant_id)
            .filter(|r| since.is_none_or(|s| r.ts >= s))
            .filter(|r| until.is_none_or(|u| r.ts <= u))
            .take(take)
            .cloned()
            .collect())
    }
}

/// In-memory `AuditVerifier` for tests. Holds a fixed verdict per tenant
/// so route tests can exercise both the success and broken-chain branches
/// without standing up Postgres.
#[derive(Debug, Default, Clone)]
pub struct StaticAuditVerifier {
    pub verdicts: std::collections::HashMap<String, VerifyReport>,
}

impl StaticAuditVerifier {
    #[must_use]
    pub fn with_verdict(tenant_id: impl Into<String>, report: VerifyReport) -> Self {
        let mut v = Self::default();
        v.verdicts.insert(tenant_id.into(), report);
        v
    }

    #[must_use]
    pub fn add(mut self, tenant_id: impl Into<String>, report: VerifyReport) -> Self {
        self.verdicts.insert(tenant_id.into(), report);
        self
    }
}

#[async_trait]
impl AuditVerifier for StaticAuditVerifier {
    async fn verify_tenant(&self, tenant_id: &str) -> Result<VerifyReport, AuditError> {
        Ok(self
            .verdicts
            .get(tenant_id)
            .cloned()
            .unwrap_or(VerifyReport::Ok { verified_count: 0 }))
    }
}

/// In-memory `AuditChainExporter` for route tests.
///
/// Holds pre-canned responses keyed by `(tenant_id, framework, format)`.
/// Route tests construct one with the bytes they want returned and verify
/// the HTTP path without standing up the full Pg adapter.
#[derive(Default)]
pub struct StaticAuditChainExporter {
    /// `Ok(bytes)` is returned verbatim; `Err(...)` is propagated.
    pub responses:
        std::collections::HashMap<(String, String, String), Result<Vec<u8>, ExportError>>,
}

impl std::fmt::Debug for StaticAuditChainExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticAuditChainExporter")
            .field("response_count", &self.responses.len())
            .finish()
    }
}

impl StaticAuditChainExporter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pre-canned response. Keys are `(tenant_id, framework, format)`.
    #[must_use]
    pub fn with(
        mut self,
        tenant_id: impl Into<String>,
        framework: impl Into<String>,
        format: impl Into<String>,
        response: Result<Vec<u8>, ExportError>,
    ) -> Self {
        self.responses.insert(
            (tenant_id.into(), framework.into(), format.into()),
            response,
        );
        self
    }
}

#[async_trait]
impl AuditChainExporter for StaticAuditChainExporter {
    async fn export(&self, req: ExportRequest) -> Result<Vec<u8>, ExportError> {
        let key = (
            req.tenant_id.clone(),
            req.framework.clone(),
            req.format.clone(),
        );
        match self.responses.get(&key) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(e)) => Err(e.clone()),
            None => Err(ExportError::InvalidArgument {
                message: format!("no canned response for {key:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, tenant: &str, ts: DateTime<Utc>) -> AuditEntryView {
        AuditEntryView {
            id,
            ts,
            tenant_id: tenant.into(),
            actor: "system".into(),
            action: "test".into(),
            resource: None,
            details: serde_json::json!({}),
            prev_hmac: "00".repeat(32),
            hmac: "ab".repeat(32),
        }
    }

    #[tokio::test]
    async fn static_reader_filters_by_tenant() {
        let t0 = Utc::now();
        let reader = StaticAuditReader::with_rows(vec![
            row(1, "t-a", t0),
            row(2, "t-b", t0),
            row(3, "t-a", t0),
        ]);
        let got = reader.list("t-a", None, None, 100).await.unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|r| r.tenant_id == "t-a"));
    }

    #[tokio::test]
    async fn static_reader_respects_limit_and_time_bounds() {
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::seconds(60);
        let reader = StaticAuditReader::with_rows(vec![row(1, "t-a", t0), row(2, "t-a", t1)]);
        let only_first = reader.list("t-a", None, Some(t0), 100).await.unwrap();
        assert_eq!(only_first.len(), 1);
        assert_eq!(only_first[0].id, 1);

        let capped = reader.list("t-a", None, None, 1).await.unwrap();
        assert_eq!(capped.len(), 1);
    }
}
