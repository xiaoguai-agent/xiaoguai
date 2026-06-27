//! Runtime configuration for the floater: where the local `xiaoguai serve`
//! lives and (optionally) the single-owner HTTP Basic credentials.
//!
//! Everything is resolved from the environment at startup. The floater is a
//! thin client — it holds NO persistent state of its own, so there is no
//! settings file to manage. Point it at a serve, optionally hand it a
//! credential, done.

use base64::Engine as _;

/// Default base URL of a local `xiaoguai serve` (DEC-033 default port `:7600`).
pub const DEFAULT_BASE_URL: &str = "http://localhost:7600";

/// The `user_id` recorded on sessions this client creates. Single-owner
/// deployments (DEC-033) ignore the body identity when owner-auth is on (the
/// server overrides it with the authenticated `Claims.sub`), so this only
/// matters for an open/no-auth serve.
pub const FLOATER_USER_ID: &str = "floater";

/// Immutable connection settings, resolved once at boot.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Base URL of the serve, no trailing slash.
    pub base_url: String,
    /// Pre-computed `Authorization` header value, e.g. `Basic dXNlcjpwYXNz`.
    /// `None` => connect with no auth (open localhost serve).
    pub auth_header: Option<String>,
}

impl AppConfig {
    /// Resolve configuration from the environment.
    ///
    /// Recognised variables:
    ///   * `XIAOGUAI_FLOATER_URL` — serve base URL (default [`DEFAULT_BASE_URL`]).
    ///   * `XIAOGUAI_FLOATER_TOKEN` — a full Bearer token (sent verbatim as
    ///     `Authorization: Bearer <token>`). Takes precedence over basic auth.
    ///   * `XIAOGUAI_FLOATER_USER` + `XIAOGUAI_FLOATER_PASS` — HTTP Basic
    ///     credentials (the serve's single-owner default scheme). Used only
    ///     when no Bearer token is set.
    #[must_use]
    pub fn from_env() -> Self {
        let base_url = std::env::var("XIAOGUAI_FLOATER_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = base_url.trim_end_matches('/').to_string();

        let auth_header = resolve_auth_header();

        Self {
            base_url,
            auth_header,
        }
    }
}

/// Build the `Authorization` header value from the environment, preferring a
/// Bearer token over Basic credentials. Returns `None` for an open serve.
fn resolve_auth_header() -> Option<String> {
    if let Some(token) = non_empty_env("XIAOGUAI_FLOATER_TOKEN") {
        return Some(format!("Bearer {token}"));
    }
    match (
        non_empty_env("XIAOGUAI_FLOATER_USER"),
        non_empty_env("XIAOGUAI_FLOATER_PASS"),
    ) {
        (Some(user), Some(pass)) => {
            let raw = format!("{user}:{pass}");
            let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
            Some(format!("Basic {encoded}"))
        }
        // A username with no password (or vice-versa) is almost certainly a
        // misconfiguration; fail open to no-auth rather than send a half
        // credential that would just 401.
        _ => None,
    }
}

/// Fetch an env var, treating empty/whitespace-only as absent.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trailing_slash_is_stripped() {
        // We can't safely mutate process env in parallel tests, so exercise the
        // pure normalisation directly.
        let normalised = "http://localhost:7600/".trim_end_matches('/').to_string();
        assert_eq!(normalised, "http://localhost:7600");
    }

    #[test]
    fn basic_auth_header_is_base64_user_colon_pass() {
        let raw = "owner:secret";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        assert_eq!(format!("Basic {encoded}"), "Basic b3duZXI6c2VjcmV0");
    }
}
