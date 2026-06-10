//! Shared PII / secret redaction primitive.
//!
//! [`redact_str`] replaces personally-identifiable and sensitive spans — email
//! addresses, `IPv4` addresses, `Bearer` tokens, AWS access-key ids, and the
//! common provider key formats (`OpenAI`/`DeepSeek` `sk-…`, Slack `xox?-…`,
//! GitHub `gh?_…` / `github_pat_…`, Google `AIza…`, secret URL query params) —
//! with stable placeholders. It lives in this leaf crate so both the audit
//! chain (`xiaoguai-audit`) and trace export (`xiaoguai-observability`) share
//! one implementation without a dependency cycle (audit already depends on
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
// SEC-17: provider key formats below — specific prefixes only, no generic
// high-entropy heuristics (those misfire on ordinary prose and hashes).
// OpenAI / DeepSeek style secret keys, incl. prefixed variants (`sk-proj-…`).
static OPENAI_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sk-[A-Za-z0-9_\-]{20,}").unwrap());
// Slack tokens (bot/app/user/legacy: xoxb-/xoxa-/xoxp-/xoxr-/xoxs-).
static SLACK_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"xox[baprs]-[A-Za-z0-9\-]{10,}").unwrap());
// GitHub tokens: classic/fine-grained prefixes (ghp_/gho_/ghu_/ghs_/ghr_) and
// the long-form `github_pat_` personal access tokens.
static GITHUB_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})").unwrap()
});
// Google API keys: fixed `AIza` prefix + 35-char body.
static GOOGLE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"AIza[A-Za-z0-9_\-]{35}").unwrap());
// Secret-bearing URL query parameters — keep the parameter name, drop the
// value (`?api_key=…`, `&token=…`, …).
static URL_QUERY_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([?&](?:key|api[_\-]?key|token|secret|access_token)=)[^&\s]+").unwrap()
});

/// Replace every PII/secret span in `input` with its placeholder.
///
/// Patterns are applied in a fixed order; `Bearer`/key tokens first so a
/// generic match cannot shadow the scheme prefix we want to keep. The URL
/// query rule runs after the key-format rules — re-matching an already
/// redacted value just rewrites the same placeholder, so the combined output
/// is stable regardless of which rule fires first. (SEC-17)
#[must_use]
pub fn redact_str(input: &str) -> String {
    let s = BEARER.replace_all(input, format!("Bearer {PLACEHOLDER_TOKEN}").as_str());
    let s = AWS_KEY.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = OPENAI_KEY.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = SLACK_TOKEN.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = GITHUB_TOKEN.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = GOOGLE_KEY.replace_all(&s, PLACEHOLDER_TOKEN);
    let s = URL_QUERY_SECRET.replace_all(&s, format!("${{1}}{PLACEHOLDER_TOKEN}").as_str());
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

    // SEC-17: provider key formats.

    #[test]
    fn redacts_openai_key() {
        assert_eq!(
            redact_str("key sk-proj-AbCdEf0123456789GhIjKl end"),
            "key [redacted-token] end"
        );
    }

    #[test]
    fn redacts_slack_token() {
        assert_eq!(
            redact_str("slack xoxb-1234567890-ABCdefGHIjkl here"),
            "slack [redacted-token] here"
        );
    }

    #[test]
    fn redacts_github_token() {
        assert_eq!(
            redact_str("pat ghp_AbCd0123456789efGhIjKl ok"),
            "pat [redacted-token] ok"
        );
        assert_eq!(
            redact_str("pat github_pat_11ABCDE_0123456789abcdefghij ok"),
            "pat [redacted-token] ok"
        );
    }

    #[test]
    fn redacts_google_key() {
        assert_eq!(
            redact_str("g AIzaSyB0123456789abcdefghijklmnopqrstuv end"),
            "g [redacted-token] end"
        );
    }

    #[test]
    fn redacts_url_query_secret_keeps_param_name() {
        assert_eq!(
            redact_str("GET https://api.example.com/v1/items?api_key=supersecret123&page=2"),
            "GET https://api.example.com/v1/items?api_key=[redacted-token]&page=2"
        );
    }

    /// Combined input — pins the rule order so overlapping patterns (URL query
    /// value that is itself a Slack token) stay stable end-to-end.
    #[test]
    fn redacts_mixed_secrets_stably() {
        let input = "Authorization: Bearer abc123 sk-proj-AbCdEf0123456789GhIjKl \
                     https://h.example.com/cb?token=xoxb-1234567890-ABCdefGHIjkl \
                     from ops@corp.io at 10.0.0.5";
        let expected = "Authorization: Bearer [redacted-token] [redacted-token] \
                        https://h.example.com/cb?token=[redacted-token] \
                        from [redacted-email] at [redacted-ip]";
        assert_eq!(redact_str(input), expected);
    }
}
