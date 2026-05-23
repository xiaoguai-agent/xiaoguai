//! v0.6.5 — bridge `xiaoguai-audit::PgAuditSink` into the api crate's
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
use xiaoguai_api::audit::{AuditEntryView, AuditError, AuditReader, AuditVerifier, VerifyReport};
use xiaoguai_audit::chain::sink::PgAuditSink;
use xiaoguai_audit::ChainError;

pub struct PgAuditAdapter {
    sink: Arc<PgAuditSink>,
}

impl PgAuditAdapter {
    #[must_use]
    pub fn new(sink: Arc<PgAuditSink>) -> Self {
        Self { sink }
    }
}

fn chain_err(e: ChainError) -> AuditError {
    AuditError::Backend(e.to_string())
}

#[async_trait]
impl AuditReader for PgAuditAdapter {
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

#[async_trait]
impl AuditVerifier for PgAuditAdapter {
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
