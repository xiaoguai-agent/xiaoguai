//! MCP OAuth refresh-token binding for the shared encryption-at-rest primitive.
//!
//! The crypto itself lives in [`xiaoguai_types::at_rest`] — a domain-neutral
//! AES-256-GCM building block also used by `xiaoguai-storage` for LLM provider
//! API keys. This module only pins the MCP-specific env-var names and the
//! refuse-to-start contract for `mcp_oauth_tokens`.
//!
//! NOTE (status): still not wired to a persistence path — the only `TokenStore`
//! today is `InMemoryTokenStore`, so no row is written to `mcp_oauth_tokens`
//! and these calls have no production caller yet. A `SqliteTokenStore` is the
//! follow-up.
//!
//! # Key management
//!
//! - Two env vars: [`ENV_KEY_CURRENT`] (current) and the optional
//!   [`ENV_KEY_PREV`] (previous, accepted on read only). Both are base64url
//!   (with or without padding) encodings of a 32-byte key.
//! - Rotation pattern: the operator generates a fresh key, moves the old key
//!   into `_PREV`, and restarts. After all tokens have refreshed at least once,
//!   the operator unsets `_PREV`.
//! - Refuse-to-start contract (enforced at the boot site, not here): if any row
//!   exists in `mcp_oauth_tokens` and the current key is absent, the server
//!   fails to boot. A fresh install with an empty table boots without keys.

pub use xiaoguai_types::at_rest::{
    AeadKey, AtRestError, Keyring, ENVELOPE_VERSION, KEY_LEN, NONCE_LEN,
};

/// Environment variable holding the current 32-byte AES-256-GCM key for MCP
/// OAuth refresh tokens, base64url-encoded (with or without `=` padding).
pub const ENV_KEY_CURRENT: &str = "XIAOGUAI_MCP_OAUTH_TOKEN_KEY";

/// Optional previous key for the rotation window. Same encoding as
/// [`ENV_KEY_CURRENT`].
pub const ENV_KEY_PREV: &str = "XIAOGUAI_MCP_OAUTH_TOKEN_KEY_PREV";

/// Load the MCP OAuth keyring from [`ENV_KEY_CURRENT`] / [`ENV_KEY_PREV`].
///
/// Unlike the opt-in [`Keyring::from_env_vars`], a missing current key here is
/// an error ([`AtRestError::KeyMissing`]) — the MCP path enforces a strict
/// refuse-to-start contract once `mcp_oauth_tokens` holds rows.
///
/// # Errors
/// - [`AtRestError::KeyMissing`] when [`ENV_KEY_CURRENT`] is unset or empty.
/// - [`AtRestError::KeyMalformed`] when a key is present but not a 32-byte
///   base64url value.
pub fn mcp_keyring_from_env() -> Result<Keyring, AtRestError> {
    Keyring::from_env_vars(ENV_KEY_CURRENT, ENV_KEY_PREV)?.ok_or(AtRestError::KeyMissing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_names_are_stable() {
        // These names are an operator-facing contract; changing them silently
        // would break existing deployments' key configuration.
        assert_eq!(ENV_KEY_CURRENT, "XIAOGUAI_MCP_OAUTH_TOKEN_KEY");
        assert_eq!(ENV_KEY_PREV, "XIAOGUAI_MCP_OAUTH_TOKEN_KEY_PREV");
    }
}
