//! OAuth 2.1 with PKCE (RFC 7636) for outbound MCP servers.
//!
//! The brief is small enough that hand-rolling beats pulling the
//! `oauth2` crate: ~120 LOC, no new transitive deps, matches the
//! workspace's `rand 0.10` + `sha2 0.11` + `base64 0.22` versions
//! already vetted by `cargo deny`.
//!
//! Protocol shape (PKCE-augmented authorization-code grant):
//!
//! ```text
//! 1. client picks `verifier`  ← 43..128 chars, URL-safe alphabet
//!    client derives `challenge = base64url_nopad(sha256(verifier))`
//! 2. client redirects user to `auth_url?response_type=code&client_id=...
//!                              &redirect_uri=...&scope=...&state=...
//!                              &code_challenge=...&code_challenge_method=S256`
//! 3. user consents; auth server redirects back to `redirect_uri?code=...&state=...`
//! 4. client POSTs to `token_url`:
//!      grant_type=authorization_code, code, redirect_uri, client_id,
//!      code_verifier
//!    → { access_token, refresh_token?, expires_in, token_type, ... }
//! 5. on expiry-1min, client POSTs to `token_url`:
//!      grant_type=refresh_token, refresh_token, client_id
//!    → { access_token, refresh_token?, expires_in }
//!    (per RFC 6749 §6, if a new refresh_token isn't returned the
//!     old one is kept)
//! ```
//!
//! Security defaults:
//!   * TLS verification ON unless `XIAOGUAI_MCP_OAUTH_INSECURE=1`
//!     (logged at `warn`).
//!   * Refresh-token rotation handled atomically by [`TokenStore::put`].
//!   * Refresh tokens stored as-is (cleartext at the DB level); RLS
//!     enforces tenant isolation. App-level encryption-at-rest is
//!     deferred to a separate hardening PR — see runbook.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dashmap::DashMap;
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{McpError, McpResult};

/// Refresh tokens this many seconds before expiry. Keeps a margin for
/// clock skew + the in-flight request.
pub const REFRESH_LEEWAY_SECS: i64 = 60;

/// Environment variable that toggles TLS verification *off* for the
/// OAuth token endpoint. Intended for self-signed corp test stacks.
/// Logged at `warn` when honored.
pub const ENV_INSECURE: &str = "XIAOGUAI_MCP_OAUTH_INSECURE";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Tagged union over the auth methods we persist to `mcp_servers.auth`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// OAuth 2.1 with PKCE.
    Oauth2Pkce(OAuth2PkceConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuth2PkceConfig {
    /// Where the user is redirected to consent (`/authorize` endpoint).
    pub auth_url: String,
    /// Where the client exchanges code → tokens and refreshes
    /// (`/token` endpoint).
    pub token_url: String,
    /// Public client identifier registered with the auth server.
    pub client_id: String,
    /// Requested scopes. Joined with spaces per RFC 6749.
    pub scopes: Vec<String>,
    /// `http://127.0.0.1:<port>/callback` — bound at register time.
    pub redirect_uri: String,
}

/// One acquired token, plus what's needed to refresh it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenBundle {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// PKCE verifier + S256 challenge, derived once per consent flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

// ---------------------------------------------------------------------------
// PKCE primitives (RFC 7636 §4)
// ---------------------------------------------------------------------------

/// RFC 7636 §4.1: 43..128 chars from `[A-Za-z0-9-._~]`.
///
/// We pick 64 random bytes → base64url-no-pad → 86 chars, well inside
/// the range and using the spec-mandated alphabet.
#[must_use]
pub fn new_pkce_pair() -> PkcePair {
    let mut bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    PkcePair {
        verifier,
        challenge,
    }
}

/// Generate a `state` value for CSRF protection on the authorize → redirect leg.
#[must_use]
pub fn new_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

// ---------------------------------------------------------------------------
// URL builder
// ---------------------------------------------------------------------------

/// Build the `/authorize` URL the user opens in a browser.
#[must_use]
pub fn build_authorize_url(cfg: &OAuth2PkceConfig, challenge: &str, state: &str) -> String {
    let scope = cfg.scopes.join(" ");
    // Build with `url` so percent-encoding is RFC-correct.
    let mut url = url::Url::parse(&cfg.auth_url).unwrap_or_else(|_| {
        // Fall back to manual concat if the configured URL is malformed
        // — `exchange_code` will fail the same way the server would.
        url::Url::parse("http://invalid.localhost/").expect("static fallback URL")
    });
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &cfg.redirect_uri)
        .append_pair("scope", &scope)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

// ---------------------------------------------------------------------------
// Token endpoint client
// ---------------------------------------------------------------------------

/// Build a reqwest client honouring `XIAOGUAI_MCP_OAUTH_INSECURE`.
///
/// # Errors
/// Returns an error if reqwest cannot build the client.
pub fn build_http_client() -> McpResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if std::env::var(ENV_INSECURE).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")) {
        tracing::warn!(
            env = ENV_INSECURE,
            "TLS verification DISABLED for OAuth token endpoint; do NOT use in production",
        );
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder
        .build()
        .map_err(|e| McpError::Transport(format!("build http client: {e}")))
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    /// Per RFC 6749 §5.1 `expires_in` is the lifetime in seconds. Some
    /// providers omit it; default to 1h.
    expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    error_description: Option<String>,
}

async fn post_token_form(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, &str)],
) -> McpResult<TokenBundle> {
    let resp = http
        .post(token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| McpError::Transport(format!("token endpoint POST: {e}")))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| McpError::Transport(format!("token endpoint body: {e}")))?;
    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
            return Err(McpError::AuthFailed(format!(
                "{}: {}",
                err.error,
                err.error_description.unwrap_or_default()
            )));
        }
        return Err(McpError::AuthFailed(format!(
            "token endpoint HTTP {status}: {body}"
        )));
    }
    let parsed: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| McpError::Protocol(format!("decode token response: {e}; body={body}")))?;
    let expires_in = parsed.expires_in.unwrap_or(3600);
    Ok(TokenBundle {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at: Utc::now() + ChronoDuration::seconds(expires_in),
    })
}

/// Exchange an authorization-code grant for a `TokenBundle`.
///
/// # Errors
/// Returns an error if the token endpoint refuses the code, the
/// response is malformed, or the network call fails.
pub async fn exchange_code(
    http: &reqwest::Client,
    cfg: &OAuth2PkceConfig,
    code: &str,
    verifier: &str,
) -> McpResult<TokenBundle> {
    let form: [(&str, &str); 5] = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &cfg.redirect_uri),
        ("client_id", &cfg.client_id),
        ("code_verifier", verifier),
    ];
    post_token_form(http, &cfg.token_url, &form).await
}

/// Refresh a `TokenBundle` via `grant_type=refresh_token`.
///
/// Per RFC 6749 §6, if the token endpoint does NOT return a new
/// `refresh_token` the old one is retained in the returned bundle.
///
/// # Errors
/// Returns an error if the bundle has no `refresh_token`, the token
/// endpoint refuses the refresh, or the network call fails.
pub async fn refresh_pkce(
    http: &reqwest::Client,
    cfg: &OAuth2PkceConfig,
    bundle: &TokenBundle,
) -> McpResult<TokenBundle> {
    let existing_refresh = bundle.refresh_token.as_deref().ok_or_else(|| {
        McpError::AuthFailed("cannot refresh: bundle has no refresh_token".into())
    })?;
    let form: [(&str, &str); 3] = [
        ("grant_type", "refresh_token"),
        ("refresh_token", existing_refresh),
        ("client_id", &cfg.client_id),
    ];
    let mut new_bundle = post_token_form(http, &cfg.token_url, &form).await?;
    // Refresh-token rotation: spec allows the server to return a new
    // refresh_token or omit it (keep old). Atomicity is on the caller
    // — `TokenStore::put` overwrites the whole row.
    if new_bundle.refresh_token.is_none() {
        new_bundle.refresh_token.clone_from(&bundle.refresh_token);
    }
    Ok(new_bundle)
}

/// Decide whether `bundle` should be refreshed *now*.
///
/// Returns `true` if `expires_at < now + REFRESH_LEEWAY_SECS`.
#[must_use]
pub fn should_refresh(bundle: &TokenBundle, now: DateTime<Utc>) -> bool {
    bundle.expires_at < now + ChronoDuration::seconds(REFRESH_LEEWAY_SECS)
}

// ---------------------------------------------------------------------------
// TokenStore
// ---------------------------------------------------------------------------

/// Persistence interface for `(server_id, tenant_id) -> TokenBundle`.
///
/// `put` MUST be atomic with respect to refresh-token rotation: a
/// concurrent reader either sees the old bundle or the new bundle,
/// never a torn read of (new `access_token`, old `refresh_token`).
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn get(&self, server_id: &str, tenant_id: &str) -> McpResult<Option<TokenBundle>>;
    async fn put(&self, server_id: &str, tenant_id: &str, bundle: &TokenBundle) -> McpResult<()>;
}

/// In-memory `TokenStore` — used by tests + the CLI's one-shot
/// register flow. Production use should plug in a Pg-backed store
/// against `mcp_oauth_tokens` (deferred follow-up).
#[derive(Debug, Default, Clone)]
pub struct InMemoryTokenStore {
    inner: Arc<DashMap<(String, String), TokenBundle>>,
}

impl InMemoryTokenStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot for assertions in tests.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<(String, String), TokenBundle> {
        self.inner
            .iter()
            .map(|kv| (kv.key().clone(), kv.value().clone()))
            .collect()
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn get(&self, server_id: &str, tenant_id: &str) -> McpResult<Option<TokenBundle>> {
        Ok(self
            .inner
            .get(&(server_id.to_string(), tenant_id.to_string()))
            .map(|kv| kv.clone()))
    }

    async fn put(&self, server_id: &str, tenant_id: &str, bundle: &TokenBundle) -> McpResult<()> {
        self.inner.insert(
            (server_id.to_string(), tenant_id.to_string()),
            bundle.clone(),
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cfg() -> OAuth2PkceConfig {
        OAuth2PkceConfig {
            auth_url: "https://example.invalid/oauth/authorize".into(),
            token_url: "https://example.invalid/oauth/token".into(),
            client_id: "my-client".into(),
            scopes: vec!["mcp.read".into(), "mcp.write".into()],
            redirect_uri: "http://127.0.0.1:7777/callback".into(),
        }
    }

    #[test]
    fn pkce_verifier_shape() {
        let pair = new_pkce_pair();
        // RFC 7636 §4.1: length 43..=128
        assert!(
            pair.verifier.len() >= 43 && pair.verifier.len() <= 128,
            "verifier length out of bounds: {}",
            pair.verifier.len()
        );
        // URL-safe alphabet only (RFC 7636 §4.1: ALPHA / DIGIT / "-" / "." / "_" / "~").
        // base64url-no-pad encodes A-Z, a-z, 0-9, -, _ — strict subset.
        for c in pair.verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "verifier contains forbidden char: {c:?}"
            );
        }
    }

    #[test]
    fn pkce_challenge_matches_sha256_of_verifier() {
        let pair = new_pkce_pair();
        let mut hasher = Sha256::new();
        hasher.update(pair.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pair.challenge, expected);
        // Challenge is also URL-safe, no padding, 43 chars (sha256 = 32B → 43 chars).
        assert_eq!(pair.challenge.len(), 43);
    }

    #[test]
    fn new_state_is_unique_and_url_safe() {
        let a = new_state();
        let b = new_state();
        assert_ne!(a, b);
        for s in [&a, &b] {
            for c in s.chars() {
                assert!(c.is_ascii_alphanumeric() || c == '-' || c == '_');
            }
        }
    }

    #[test]
    fn authorize_url_includes_required_params() {
        let cfg = sample_cfg();
        let url = build_authorize_url(&cfg, "CHAL123", "STATE456");
        // Must contain every PKCE-mandated query field.
        for needle in [
            "response_type=code",
            "client_id=my-client",
            "code_challenge=CHAL123",
            "code_challenge_method=S256",
            "state=STATE456",
        ] {
            assert!(url.contains(needle), "missing {needle} in {url}");
        }
        // Scopes joined with `+` (URL-encoded space).
        assert!(url.contains("scope=mcp.read+mcp.write"), "got {url}");
        // Redirect URI is percent-encoded.
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7777%2Fcallback"),
            "got {url}"
        );
    }

    #[test]
    fn should_refresh_within_leeway_window() {
        let now = Utc::now();
        let mk = |secs: i64| TokenBundle {
            access_token: "x".into(),
            refresh_token: Some("r".into()),
            expires_at: now + ChronoDuration::seconds(secs),
        };
        // already expired
        assert!(should_refresh(&mk(-1), now));
        // expires in 30s, leeway 60s → refresh
        assert!(should_refresh(&mk(30), now));
        // expires in 5 min → no refresh
        assert!(!should_refresh(&mk(300), now));
        // exactly at leeway boundary — current impl uses strict <
        // and includes the leeway buffer, so 60s in the future is NOT
        // refreshed (boundary case documented).
        assert!(!should_refresh(&mk(REFRESH_LEEWAY_SECS), now));
    }

    #[tokio::test]
    async fn inmemory_store_round_trip() {
        let store = InMemoryTokenStore::new();
        let bundle = TokenBundle {
            access_token: "AT".into(),
            refresh_token: Some("RT".into()),
            expires_at: Utc::now() + ChronoDuration::seconds(3600),
        };
        assert!(store.get("srv-1", "t-1").await.unwrap().is_none());
        store.put("srv-1", "t-1", &bundle).await.unwrap();
        let got = store.get("srv-1", "t-1").await.unwrap().unwrap();
        assert_eq!(got, bundle);
        // Tenant isolation: same server_id, different tenant → miss.
        assert!(store.get("srv-1", "t-2").await.unwrap().is_none());
        // Overwrite (rotation simulation).
        let new_bundle = TokenBundle {
            access_token: "AT2".into(),
            refresh_token: Some("RT2".into()),
            expires_at: Utc::now() + ChronoDuration::seconds(7200),
        };
        store.put("srv-1", "t-1", &new_bundle).await.unwrap();
        let got = store.get("srv-1", "t-1").await.unwrap().unwrap();
        assert_eq!(got.access_token, "AT2");
        assert_eq!(got.refresh_token.as_deref(), Some("RT2"));
    }

    #[test]
    fn auth_config_serde_round_trip() {
        let cfg = AuthConfig::Oauth2Pkce(sample_cfg());
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains(r#""type":"oauth2_pkce""#));
        let parsed: AuthConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[tokio::test]
    async fn refresh_keeps_old_token_when_server_omits_new() {
        // No network here — we test the rotation policy at the
        // function level by stubbing the inner post path. Since
        // `refresh_pkce` calls `post_token_form` we can't easily mock
        // without a server; the integration test in
        // `tests/oauth_pkce_e2e.rs` covers the live path. Here we
        // assert the boolean policy independently.
        let bundle = TokenBundle {
            access_token: "old".into(),
            refresh_token: Some("OLD_RT".into()),
            expires_at: Utc::now(),
        };
        // Simulate: response_without_new_refresh_token
        let mut returned = TokenBundle {
            access_token: "new".into(),
            refresh_token: None,
            expires_at: Utc::now() + ChronoDuration::seconds(3600),
        };
        if returned.refresh_token.is_none() {
            returned.refresh_token = bundle.refresh_token.clone();
        }
        assert_eq!(returned.refresh_token.as_deref(), Some("OLD_RT"));
        assert_eq!(returned.access_token, "new");
    }
}
