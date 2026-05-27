//! PII / secret redaction for audit entries.
//!
//! Scrubs personally-identifiable and sensitive substrings — email addresses,
//! `IPv4` addresses, `Bearer` tokens, and AWS access-key ids — from an
//! [`AuditEntry`] **before** it is HMAC-signed. Because the persisted row and
//! its signature are both computed over the redacted form,
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

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::AuditEntry;

const PLACEHOLDER_EMAIL: &str = "[redacted-email]";
const PLACEHOLDER_IP: &str = "[redacted-ip]";
const PLACEHOLDER_TOKEN: &str = "[redacted-token]";

// Patterns are conservative — tuned to minimise false positives on audit data.
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap());
static IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap());
// `Bearer <token>` (case-insensitive) — keeps the scheme word, drops the token.
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]+").unwrap());
// AWS access-key id.
static AWS_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"AKIA[0-9A-Z]{16}").unwrap());

/// Replace every PII/secret span in `input` with its placeholder.
///
/// Patterns are applied in a fixed order; `Bearer`/key tokens first so a
/// generic match can't shadow the scheme prefix we want to keep.
#[must_use]
pub fn redact_str(input: &str) -> String {
    let s = BEARER.replace_all(input, format!("Bearer {PLACEHOLDER_TOKEN}").as_str());
    let s = AWS_KEY.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = EMAIL.replace_all(&s, PLACEHOLDER_EMAIL);
    let s = IPV4.replace_all(&s, PLACEHOLDER_IP);
    s.into_owned()
}

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
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn redacts_email() {
        assert_eq!(
            redact_str("contact alice@example.com now"),
            "contact [redacted-email] now"
        );
    }

    #[test]
    fn redacts_ipv4() {
        assert_eq!(
            redact_str("from 10.0.1.5 inbound"),
            "from [redacted-ip] inbound"
        );
    }

    #[test]
    fn redacts_bearer_keeps_scheme() {
        assert_eq!(
            redact_str("Authorization: Bearer abc.DEF-123"),
            "Authorization: Bearer [redacted-token]"
        );
    }

    #[test]
    fn redacts_aws_key() {
        assert_eq!(
            redact_str("key AKIAIOSFODNN7EXAMPLE end"),
            "key [redacted-token] end"
        );
    }

    #[test]
    fn leaves_clean_text_unchanged() {
        let clean = "session.create for project alpha";
        assert_eq!(redact_str(clean), clean);
    }

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
