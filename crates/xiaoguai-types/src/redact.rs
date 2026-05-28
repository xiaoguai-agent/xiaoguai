//! Shared PII / secret redaction primitive.
//!
//! [`redact_str`] replaces personally-identifiable and sensitive spans — email
//! addresses, `IPv4` addresses, `Bearer` tokens, and AWS access-key ids — with
//! stable placeholders. It lives in this leaf crate so both the audit chain
//! (`xiaoguai-audit`) and trace export (`xiaoguai-observability`) share one
//! implementation without a dependency cycle (audit already depends on
//! observability, so observability cannot depend back on audit).

use std::sync::LazyLock;

use regex::Regex;

const PLACEHOLDER_EMAIL: &str = "[redacted-email]";
const PLACEHOLDER_IP: &str = "[redacted-ip]";
const PLACEHOLDER_TOKEN: &str = "[redacted-token]";

// Patterns are conservative — tuned to minimise false positives on real data.
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
/// generic match cannot shadow the scheme prefix we want to keep.
#[must_use]
pub fn redact_str(input: &str) -> String {
    let s = BEARER.replace_all(input, format!("Bearer {PLACEHOLDER_TOKEN}").as_str());
    let s = AWS_KEY.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = EMAIL.replace_all(&s, PLACEHOLDER_EMAIL);
    let s = IPV4.replace_all(&s, PLACEHOLDER_IP);
    s.into_owned()
}

#[cfg(test)]
mod tests {
    use super::redact_str;

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
}
