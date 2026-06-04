//! HMAC-chained audit sink adapter for HOTL decisions.
//!
//! Wraps `xiaoguai_audit::SqliteAuditSink::append` behind a trait so the API
//! crate doesn't depend directly on the audit sink in tests and so the
//! HOTL decision route can audit `hotl.decision` events without coupling to
//! the read-only `state.audit: Option<Arc<dyn AuditReader>>` field.
//!
//! Audit failures must NOT block the decision — callers discard the result
//! with `.ok()` after best-effort logging.

use async_trait::async_trait;
use xiaoguai_audit::AuditEntry;

/// Append-only audit interface used by HOTL routes.
///
/// Production wires `SqliteAuditSink` (which already implements `append` with
/// redaction + HMAC chaining); tests provide an in-memory implementation
/// that captures entries for assertion.
#[async_trait]
pub trait HotlAuditSink: Send + Sync + std::fmt::Debug {
    /// Append a single entry. Returns `Ok(())` on success; the concrete
    /// error type is intentionally opaque (a `String`) so we don't leak
    /// `xiaoguai_audit::ChainError` through the trait surface.
    async fn append(&self, entry: AuditEntry) -> Result<(), String>;
}

// ── in-memory implementation (tests) ─────────────────────────────────────────

/// In-memory sink that captures every appended entry. Used by integration
/// tests to assert that decision routes emit `hotl.decision` audit lines.
#[derive(Debug, Default)]
pub struct InMemoryHotlAuditSink {
    inner: parking_lot::Mutex<Vec<AuditEntry>>,
}

impl InMemoryHotlAuditSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only snapshot of all captured entries.
    #[must_use]
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.inner.lock().clone()
    }
}

#[async_trait]
impl HotlAuditSink for InMemoryHotlAuditSink {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.inner.lock().push(entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn captures_entry() {
        let sink = InMemoryHotlAuditSink::new();
        let entry = AuditEntry {
            ts: Utc::now(),
            tenant_id: "ten_a".into(),
            actor: "alice".into(),
            action: "hotl.decision".into(),
            resource: Some("escalation:abc".into()),
            details: serde_json::json!({"verdict": "allow"}),
        };
        sink.append(entry.clone()).await.unwrap();
        let snap = sink.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].action, "hotl.decision");
    }
}
