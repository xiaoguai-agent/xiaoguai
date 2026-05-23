//! Audit linkage seam.
//!
//! The scheduler is required by design (roadmap §5.3) to write an
//! `audit_log` row for every `JobRun`. We don't depend on
//! `xiaoguai-storage` directly — it would create a circular crate
//! graph once `xiaoguai-storage` learns about scheduled-job tables.
//! Instead the runner takes an [`AuditAppender`] trait object;
//! `xiaoguai-core` wires a thin shim around `PgAuditSink`.
//!
//! The trait imports [`xiaoguai_audit::AuditEntry`] so the wire shape
//! matches what `PgAuditSink::append` already accepts.

use async_trait::async_trait;
use parking_lot::Mutex;
use xiaoguai_audit::AuditEntry;

#[async_trait]
pub trait AuditAppender: Send + Sync {
    async fn append(&self, entry: AuditEntry) -> Result<(), String>;
}

/// No-op appender. Useful when audit is disabled (single-binary dev
/// runs) or for unit tests that don't care about audit linkage.
#[derive(Debug, Default, Clone)]
pub struct NullAuditAppender;

#[async_trait]
impl AuditAppender for NullAuditAppender {
    async fn append(&self, _entry: AuditEntry) -> Result<(), String> {
        Ok(())
    }
}

/// Test appender that captures every entry in memory.
#[derive(Default)]
pub struct RecordingAuditAppender {
    entries: Mutex<Vec<AuditEntry>>,
}

impl RecordingAuditAppender {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.lock().clone()
    }
}

#[async_trait]
impl AuditAppender for RecordingAuditAppender {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.entries.lock().push(entry);
        Ok(())
    }
}
