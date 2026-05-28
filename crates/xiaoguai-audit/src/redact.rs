//! PII / secret redaction for audit entries.
//!
//! Wraps the shared [`xiaoguai_types::redact_str`] primitive (email, `IPv4`,
//! `Bearer` token, and AWS access-key scrubbing) with audit-specific logic:
//! recursing into a JSON `details` payload and producing a redacted
//! [`AuditEntry`].
//!
//! Redaction runs **before** an entry is HMAC-signed, so the persisted row and
//! its signature are both computed over the redacted form and
//! [`ChainedAudit::verify_chain`](crate::ChainedAudit::verify_chain) stays valid.
//!
//! Redaction is *immutable*: [`Redactor::redact`] returns a new [`AuditEntry`];
//! the input is never mutated. Two fields are deliberately **never** redacted:
//!
//! * `tenant_id` — scopes the per-tenant chain (the sink queries by it); a
//!   redacted tenant would orphan the chain.
//! * `action` — a fixed verb (`session.create`, `tool.invoke`, …), not PII.
//!
//! Only `actor`, `resource`, and string values nested inside `details` are
//! scrubbed (JSON object keys are preserved so structure is intact).

use serde_json::Value;
use xiaoguai_types::redact_str;

use crate::AuditEntry;

/// Recursively redact every string *value* in a JSON document. Object keys and
/// non-string scalars are left untouched.
fn redact_json(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_str(s)),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_json(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Stateless PII/secret redactor for [`AuditEntry`] values.
#[derive(Debug, Clone, Copy, Default)]
pub struct Redactor;

impl Redactor {
    /// Build a redactor with the default pattern set.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Return a redacted copy of `entry`. The input is not mutated.
    ///
    /// `tenant_id` and `action` pass through unchanged (see module docs);
    /// `actor`, `resource`, and `details` are scrubbed.
    #[must_use]
    pub fn redact(&self, entry: AuditEntry) -> AuditEntry {
        AuditEntry {
            ts: entry.ts,
            tenant_id: entry.tenant_id,
            actor: redact_str(&entry.actor),
            action: entry.action,
            resource: entry.resource.as_deref().map(redact_str),
            details: redact_json(&entry.details),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{redact_json, Redactor};
    use crate::AuditEntry;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn redacts_nested_json_values_not_keys() {
        let input = json!({
            "email": "bob@corp.io",
            "nested": { "host": "192.168.0.1", "note": "ok" },
            "list": ["carol@x.org", "fine"],
            "count": 42
        });
        let out = redact_json(&input);
        assert_eq!(out["email"], json!("[redacted-email]"));
        assert_eq!(out["nested"]["host"], json!("[redacted-ip]"));
        assert_eq!(out["nested"]["note"], json!("ok"));
        assert_eq!(out["list"][0], json!("[redacted-email]"));
        assert_eq!(out["list"][1], json!("fine"));
        assert_eq!(out["count"], json!(42));
    }

    #[test]
    fn redact_entry_preserves_tenant_and_action() {
        let entry = AuditEntry {
            ts: Utc::now(),
            tenant_id: "tenant-a@keep.me".to_string(),
            actor: "user:dave@example.com".to_string(),
            action: "session.create".to_string(),
            resource: Some("/inbox/eve@example.com".to_string()),
            details: json!({"ip": "10.1.2.3"}),
        };
        let red = Redactor::new().redact(entry);
        // tenant_id + action pass through verbatim.
        assert_eq!(red.tenant_id, "tenant-a@keep.me");
        assert_eq!(red.action, "session.create");
        // actor / resource / details are scrubbed.
        assert_eq!(red.actor, "user:[redacted-email]");
        assert_eq!(red.resource.as_deref(), Some("/inbox/[redacted-email]"));
        assert_eq!(red.details["ip"], json!("[redacted-ip]"));
    }
}
