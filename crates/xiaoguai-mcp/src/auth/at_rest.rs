//! AES-256-GCM encryption-at-rest for outbound-MCP refresh tokens.
//!
//! # Threat model
//!
//! `mcp_oauth_tokens.refresh_token` is the long-lived credential for an
//! outbound MCP server. Up to and including PR #73 / DEC-015 it was stored
//! cleartext in Postgres — RLS provided tenant-isolation but a Postgres
//! backup leak or read-replica compromise meant the refresh tokens were
//! exfiltrable. This module closes that gap: refresh tokens are
//! authenticated-encrypted with a key the operator supplies out-of-band
//! and which the DB has never seen.
//!
//! # Key management
//!
//! - Two env vars: `XIAOGUAI_MCP_OAUTH_TOKEN_KEY` (current) and the
//!   optional `XIAOGUAI_MCP_OAUTH_TOKEN_KEY_PREV` (previous, accepted on
//!   read only). Both are base64url (with or without padding) encodings
//!   of a 32-byte key.
//! - Rotation pattern: at rotation time the operator generates a fresh
//!   key, moves the old key into `_PREV`, and restarts. New encryptions
//!   use the new key; existing rows decrypt against either. After all
//!   tokens have refreshed at least once (= rotated past, since
//!   `TokenStore::put` rewrites on every refresh), the operator unsets
//!   `_PREV`.
//! - Refuse-to-start contract (enforced at the boot site, not here):
//!   if any row exists in `mcp_oauth_tokens` and neither env var is
//!   present, the server fails to boot with a clear message. A fresh
//!   install with an empty table is allowed to boot without keys.
//!
//! # Wire format
//!
//! The ciphertext stored in `refresh_token_encrypted BYTEA` is the
//! concatenation `version_byte || nonce(12B) || ct_with_tag`. Versioning
//! is for future migration (e.g. ChaCha20-Poly1305). Today version = 1.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use rand::Rng as _;
use thiserror::Error;

/// Environment variable holding the current 32-byte AES-256-GCM key,
/// base64url-encoded (with or without `=` padding).
pub const ENV_KEY_CURRENT: &str = "XIAOGUAI_MCP_OAUTH_TOKEN_KEY";

/// Optional previous key for the rotation window. Same encoding as
/// [`ENV_KEY_CURRENT`].
pub const ENV_KEY_PREV: &str = "XIAOGUAI_MCP_OAUTH_TOKEN_KEY_PREV";

/// Byte length of an AES-256-GCM key.
pub const KEY_LEN: usize = 32;

/// Byte length of the AES-GCM nonce. 96-bit per NIST SP 800-38D.
pub const NONCE_LEN: usize = 12;

/// First byte of the ciphertext envelope. Version 1 = AES-256-GCM,
/// 12B random nonce, AEAD over empty AAD.
pub const ENVELOPE_VERSION: u8 = 1;

/// Errors raised by the encryption-at-rest layer.
#[derive(Debug, Error)]
pub enum AtRestError {
    /// Env var is missing AND the caller insists a keyring is required.
    #[error("XIAOGUAI_MCP_OAUTH_TOKEN_KEY is required but not set")]
    KeyMissing,

    /// An env var was present but did not decode as a 32-byte base64url
    /// value.
    #[error("encryption key {var} is malformed: {reason}")]
    KeyMalformed { var: &'static str, reason: String },

    /// AEAD decryption failed against every key in the keyring.
    #[error("ciphertext decryption failed against every available key")]
    Decrypt,

    /// Ciphertext envelope was too short or had an unknown version byte.
    #[error("ciphertext envelope is malformed: {0}")]
    Envelope(&'static str),

    /// Plaintext was not valid UTF-8 after decryption.
    #[error("decrypted bytes are not valid UTF-8")]
    NotUtf8,

    /// AEAD encryption failed.
    #[error("ciphertext encryption failed")]
    Encrypt,
}

/// A 32-byte AES-256-GCM key, materialised in memory.
#[derive(Debug, Clone)]
pub struct AeadKey(pub [u8; KEY_LEN]);

impl AeadKey {
    /// Parse a base64url-encoded 32-byte key. Accepts both padded and
    /// unpadded forms.
    ///
    /// # Errors
    /// Returns [`AtRestError::KeyMalformed`] if the input does not decode
    /// to exactly [`KEY_LEN`] bytes.
    pub fn from_b64url(s: &str, env_name: &'static str) -> Result<Self, AtRestError> {
        let trimmed = s.trim();
        let decoded = URL_SAFE_NO_PAD
            .decode(trimmed)
            .or_else(|_| URL_SAFE.decode(trimmed))
            .map_err(|e| AtRestError::KeyMalformed {
                var: env_name,
                reason: format!("base64url decode failed: {e}"),
            })?;
        if decoded.len() != KEY_LEN {
            return Err(AtRestError::KeyMalformed {
                var: env_name,
                reason: format!(
                    "expected {KEY_LEN} bytes after decode, got {}",
                    decoded.len()
                ),
            });
        }
        let mut buf = [0u8; KEY_LEN];
        buf.copy_from_slice(&decoded);
        Ok(Self(buf))
    }

    fn as_gcm_key(&self) -> &Key<Aes256Gcm> {
        Key::<Aes256Gcm>::from_slice(&self.0)
    }
}

/// Set of acceptable keys for encryption / decryption.
#[derive(Debug, Clone)]
pub struct Keyring {
    current: AeadKey,
    prev: Option<AeadKey>,
}

impl Keyring {
    /// Construct a keyring from already-decoded keys.
    #[must_use]
    pub fn with_keys(current: AeadKey, prev: Option<AeadKey>) -> Self {
        Self { current, prev }
    }

    /// Load a keyring from the environment.
    ///
    /// # Errors
    /// Returns [`AtRestError::KeyMissing`] when the current env var is
    /// absent, or [`AtRestError::KeyMalformed`] when either is present
    /// but not a 32-byte base64url value.
    pub fn from_env() -> Result<Self, AtRestError> {
        let current_raw = std::env::var(ENV_KEY_CURRENT)
            .ok()
            .filter(|s| !s.trim().is_empty());
        let prev_raw = std::env::var(ENV_KEY_PREV)
            .ok()
            .filter(|s| !s.trim().is_empty());

        let Some(cur) = current_raw else {
            return Err(AtRestError::KeyMissing);
        };
        let current = AeadKey::from_b64url(&cur, ENV_KEY_CURRENT)?;
        let prev = match prev_raw {
            Some(s) => Some(AeadKey::from_b64url(&s, ENV_KEY_PREV)?),
            None => None,
        };
        Ok(Self { current, prev })
    }

    /// Encrypt a plaintext refresh token. Returns the framed envelope
    /// `[version || nonce || ct]`.
    ///
    /// # Errors
    /// Returns [`AtRestError::Encrypt`] if the underlying AEAD call fails.
    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, AtRestError> {
        let cipher = Aes256Gcm::new(self.current.as_gcm_key());
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: b"",
                },
            )
            .map_err(|_| AtRestError::Encrypt)?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
        out.push(ENVELOPE_VERSION);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt a framed envelope, trying the current key first then the
    /// previous one if present.
    ///
    /// # Errors
    /// - [`AtRestError::Envelope`] if envelope is shorter than header
    ///   or has an unknown version byte.
    /// - [`AtRestError::Decrypt`] if all keys fail.
    /// - [`AtRestError::NotUtf8`] if decrypted bytes are not UTF-8.
    pub fn decrypt(&self, envelope: &[u8]) -> Result<String, AtRestError> {
        if envelope.len() < 1 + NONCE_LEN {
            return Err(AtRestError::Envelope("ciphertext shorter than header"));
        }
        if envelope[0] != ENVELOPE_VERSION {
            return Err(AtRestError::Envelope("unknown envelope version byte"));
        }
        let nonce = Nonce::from_slice(&envelope[1..1 + NONCE_LEN]);
        let ct = &envelope[1 + NONCE_LEN..];

        let candidates = std::iter::once(&self.current).chain(self.prev.as_ref());
        for key in candidates {
            let cipher = Aes256Gcm::new(key.as_gcm_key());
            if let Ok(pt) = cipher.decrypt(nonce, Payload { msg: ct, aad: b"" }) {
                return String::from_utf8(pt).map_err(|_| AtRestError::NotUtf8);
            }
        }
        Err(AtRestError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_key(byte: u8) -> AeadKey {
        AeadKey([byte; KEY_LEN])
    }

    fn keyring_single(byte: u8) -> Keyring {
        Keyring::with_keys(raw_key(byte), None)
    }

    #[test]
    fn round_trip_ascii_token() {
        let kr = keyring_single(0xAA);
        let ct = kr.encrypt("refresh-token-secret-12345").unwrap();
        let pt = kr.decrypt(&ct).unwrap();
        assert_eq!(pt, "refresh-token-secret-12345");
    }

    #[test]
    fn round_trip_utf8_token() {
        let kr = keyring_single(0x42);
        let pt_in = "刷新令牌-αβγ-🔐";
        let ct = kr.encrypt(pt_in).unwrap();
        assert_eq!(kr.decrypt(&ct).unwrap(), pt_in);
    }

    #[test]
    fn ciphertext_includes_version_and_nonce_header() {
        let kr = keyring_single(0x01);
        let ct = kr.encrypt("x").unwrap();
        assert_eq!(ct[0], ENVELOPE_VERSION);
        assert!(ct.len() >= 1 + NONCE_LEN + 16);
    }

    #[test]
    fn nonces_are_random_across_calls() {
        let kr = keyring_single(0x77);
        let a = kr.encrypt("hello").unwrap();
        let b = kr.encrypt("hello").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn rotation_decrypts_with_previous_key() {
        let old = raw_key(0x10);
        let new = raw_key(0x20);
        let kr_old = Keyring::with_keys(old.clone(), None);
        let ct = kr_old.encrypt("rotated").unwrap();
        let kr_rotated = Keyring::with_keys(new, Some(old));
        assert_eq!(kr_rotated.decrypt(&ct).unwrap(), "rotated");
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let kr_a = keyring_single(0x55);
        let ct = kr_a.encrypt("secret").unwrap();
        let kr_b = keyring_single(0x66);
        assert!(matches!(kr_b.decrypt(&ct).unwrap_err(), AtRestError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_fails_with_gcm_tag_check() {
        let kr = keyring_single(0x99);
        let mut ct = kr.encrypt("auditable").unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0x01;
        assert!(matches!(kr.decrypt(&ct).unwrap_err(), AtRestError::Decrypt));
    }

    #[test]
    fn truncated_envelope_returns_envelope_error() {
        let kr = keyring_single(0x12);
        let too_short = vec![ENVELOPE_VERSION, 0, 0, 0];
        assert!(matches!(
            kr.decrypt(&too_short).unwrap_err(),
            AtRestError::Envelope(_)
        ));
    }

    #[test]
    fn unknown_version_byte_rejected() {
        let kr = keyring_single(0x13);
        let mut bad = kr.encrypt("hi").unwrap();
        bad[0] = 0xFF;
        assert!(matches!(
            kr.decrypt(&bad).unwrap_err(),
            AtRestError::Envelope(_)
        ));
    }

    #[test]
    fn from_b64url_accepts_padded_and_unpadded() {
        let raw = [0xABu8; KEY_LEN];
        let padded = URL_SAFE.encode(raw);
        let unpadded = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(AeadKey::from_b64url(&padded, ENV_KEY_CURRENT).unwrap().0, raw);
        assert_eq!(AeadKey::from_b64url(&unpadded, ENV_KEY_CURRENT).unwrap().0, raw);
    }

    #[test]
    fn from_b64url_rejects_wrong_length() {
        let short = URL_SAFE_NO_PAD.encode([0u8; 16]);
        assert!(matches!(
            AeadKey::from_b64url(&short, ENV_KEY_CURRENT).unwrap_err(),
            AtRestError::KeyMalformed { .. }
        ));
    }

    #[test]
    fn from_b64url_rejects_non_base64() {
        assert!(matches!(
            AeadKey::from_b64url("@@@not-base64@@@", ENV_KEY_CURRENT).unwrap_err(),
            AtRestError::KeyMalformed { .. }
        ));
    }
}
