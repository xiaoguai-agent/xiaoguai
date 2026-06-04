//! v0.6.5 — bridge `xiaoguai-audit::SqliteAuditSink` into the api crate's
//! `AuditReader` + `AuditVerifier` traits.
//!
//! Lives in `xiaoguai-core` because the api crate intentionally doesn't
//! depend on `xiaoguai-audit` (keeps the API layer concrete-storage-
//! agnostic). The bridge wraps a single sink so the same chain engine
//! services both endpoints — no risk of two sinks computing HMACs with
//! divergent keys.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use xiaoguai_api::audit::{
    AuditChainExporter, AuditEntryView, AuditError, AuditReader, AuditVerifier,
    ExportError as ApiExportError, ExportRequest, VerifyReport,
};
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_audit::{
    export_bundle as audit_export_bundle, render as audit_render, ChainError, ExportError,
    ExportWindow, Format, Framework,
};

pub struct SqliteAuditAdapter {
    sink: Arc<SqliteAuditSink>,
}

impl SqliteAuditAdapter {
    #[must_use]
    pub fn new(sink: Arc<SqliteAuditSink>) -> Self {
        Self { sink }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as `.map_err(chain_err)` — changing to `&e` would require closure wrappers at every call site"
)]
fn chain_err(e: ChainError) -> AuditError {
    AuditError::Backend(e.to_string())
}

#[async_trait]
impl AuditReader for SqliteAuditAdapter {
    async fn list(
        &self,
        tenant_id: &str,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<AuditEntryView>, AuditError> {
        let rows = self
            .sink
            .list(tenant_id, since, until, limit)
            .await
            .map_err(chain_err)?;
        Ok(rows
            .into_iter()
            .map(|s| AuditEntryView {
                id: s.id,
                ts: s.entry.ts,
                tenant_id: s.entry.tenant_id,
                actor: s.entry.actor,
                action: s.entry.action,
                resource: s.entry.resource,
                details: s.entry.details,
                prev_hmac: hex::encode(s.prev_hmac),
                hmac: hex::encode(s.hmac),
            })
            .collect())
    }
}

/// Bound on the row count pulled per export. Streaming export is out of
/// scope for T5 — see the runbook for the rationale. Production tenants
/// with >100k events in a window should request shorter windows.
const EXPORT_ROW_CAP: i64 = 100_000;

#[async_trait]
impl AuditChainExporter for SqliteAuditAdapter {
    async fn export(&self, req: ExportRequest) -> Result<Vec<u8>, ApiExportError> {
        let framework =
            Framework::parse(&req.framework).map_err(|s| ApiExportError::InvalidArgument {
                message: format!("unknown framework: {s}"),
            })?;
        let format = Format::parse(&req.format).map_err(|s| ApiExportError::InvalidArgument {
            message: format!("unknown format: {s}"),
        })?;
        let window =
            ExportWindow::new(req.from, req.to).ok_or_else(|| ApiExportError::InvalidArgument {
                message: "from must be <= to".into(),
            })?;

        let rows = self
            .sink
            .list(&req.tenant_id, Some(req.from), Some(req.to), EXPORT_ROW_CAP)
            .await
            .map_err(|e| ApiExportError::Backend {
                message: e.to_string(),
            })?;

        let bundle = audit_export_bundle(framework, req.tenant_id, rows, window, self.sink.chain())
            .map_err(map_export_err)?;

        audit_render(&bundle, format).map_err(map_export_err)
    }
}

fn map_export_err(e: ExportError) -> ApiExportError {
    match e {
        ExportError::ChainBroken {
            first_broken_id,
            first_broken_ts,
        } => ApiExportError::ChainBroken {
            first_broken_id,
            first_broken_ts,
        },
        ExportError::PdfUnimplemented => ApiExportError::PdfUnimplemented,
        ExportError::Chain { message } => ApiExportError::Backend { message },
    }
}

#[async_trait]
impl AuditVerifier for SqliteAuditAdapter {
    async fn verify_tenant(&self, tenant_id: &str) -> Result<VerifyReport, AuditError> {
        // We need a row count for the success case; pull the same list
        // (`verify_tenant` would walk it twice otherwise). For broken
        // chains we surface the offending row id from the error variant.
        let entries = self
            .sink
            .list(tenant_id, None, None, i64::MAX)
            .await
            .map_err(chain_err)?;
        let verified_count = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let zero = [0u8; 32];
        match self.sink.chain().verify_chain(&zero, &entries) {
            Ok(()) => Ok(VerifyReport::Ok { verified_count }),
            Err(ChainError::HmacMismatch(id) | ChainError::LinkBroken(_, id)) => {
                Ok(VerifyReport::Broken { broken_at: id })
            }
            Err(e) => Err(chain_err(e)),
        }
    }
}
