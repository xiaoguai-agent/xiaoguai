//! Shared best-effort audit-append helper (Phase C / DEC-041).
//!
//! The block that builds a single-owner `xiaoguai_audit::AuditEntry` and
//! appends it to the feature-generic `team_audit` sink — logging on failure and
//! never propagating, since the operation is already persisted — was copy-pasted
//! across the teams / incidents / orchestrate / memory route handlers and the
//! incident pipeline. It lives here once now; callers keep their thin
//! domain-named wrappers (`audit` / `audit_memory`) for call-site readability.

use std::sync::Arc;

use chrono::Utc;

use crate::hotl::audit::HotlAuditSink;

/// Append a best-effort `owner` audit entry to `sink`. A `None` sink (unwired in
/// tests / minimal deployments) is a no-op; an append failure is logged and
/// discarded — telemetry must never roll back a persisted operation.
pub async fn audit_event(
    sink: &Option<Arc<dyn HotlAuditSink>>,
    action: &str,
    resource: String,
    details: serde_json::Value,
) {
    let Some(sink) = sink else { return };
    let entry = xiaoguai_audit::AuditEntry {
        ts: Utc::now(),
        tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
        actor: "owner".to_string(),
        action: action.to_string(),
        resource: Some(resource),
        details,
    };
    if let Err(e) = sink.append(entry).await {
        tracing::warn!(error = %e, action, "audit append failed (non-blocking)");
    }
}
