//! Outbound auth methods for MCP HTTP transports.
//!
//! Tier-3 T4 (2026-05-29) ships [`oauth2_pkce`]: OAuth 2.1 + PKCE per
//! RFC 7636, plus per-tenant token persistence via the [`TokenStore`]
//! trait. The bearer-string path through
//! [`HttpClientConfig::auth_header`] remains the simple case.
//!
//! Sprint-8 S8-5 (2026-05-29) closes the encryption-at-rest gap via
//! [`at_rest`]: AES-256-GCM with a dual-key rotation window. Refresh
//! tokens are encrypted before they hit the `mcp_oauth_tokens` table.
//!
//! Out of scope (documented in `docs/runbooks/outbound-mcp-oauth.md`):
//!   * RFC 7591 dynamic client registration
//!   * RFC 8628 device-code flow
//!   * mTLS client auth
//!   * RFC 7662 token introspection
//!   * UI for token management

pub mod at_rest;
pub mod oauth2_pkce;

pub use at_rest::{
    AeadKey, AtRestError, Keyring, ENV_KEY_CURRENT, ENV_KEY_PREV, ENVELOPE_VERSION, KEY_LEN,
    NONCE_LEN,
};
pub use oauth2_pkce::{
    build_authorize_url, exchange_code, new_pkce_pair, new_state, refresh_pkce, should_refresh,
    AuthConfig, InMemoryTokenStore, OAuth2PkceConfig, PkcePair, TokenBundle, TokenStore,
    REFRESH_LEEWAY_SECS,
};
