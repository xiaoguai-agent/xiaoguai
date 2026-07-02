//! AES-256-GCM authenticated encryption-at-rest primitive.
//!
//! A small, domain-neutral building block for encrypting individual secret
//! fields before they are written to the embedded `SQLite` store — e.g. outbound
//! MCP OAuth refresh tokens (`xiaoguai-mcp`) or LLM provider API keys
//! (`xiaoguai-storage`). Each consuming domain binds its own env-var names for
//! the key material via [`Keyring::from_env_vars`]; this module only knows how
//! to parse keys, encrypt, and decrypt.
//!
//! # Threat model
//!
//! A secret stored cleartext at rest is exfiltrable via a database-backup leak
//! or a file-system compromise. This primitive closes that gap: the plaintext
//! is authenticated-encrypted under a 32-byte key the operator supplies
//! out-of-band (an env var) and that the database has never seen.
//!
//! # Key management
//!
//! Two keys form a rotation window: a *current* key (used for new encryptions
//! and accepted on read) and an optional *previous* key (accepted on read
//! only). Rotation: generate a fresh key, move the old one into the `_PREV`
//! slot, restart. New encryptions use the new key; existing ciphertexts
//! decrypt against either. After every ciphertext has been rewritten under the
//! new key, the operator drops `_PREV`. The env-var *names* are chosen by the
//! consuming domain, not baked in here.
//!
//! # Wire format
//!
//! The framed envelope is `version_byte || nonce(12B) || ct_with_tag`. The
//! version byte (today `1` = AES-256-GCM, 12-byte random nonce, AEAD over empty
//! AAD) reserves room for a future migration (e.g. `ChaCha20-Poly1305`). Callers
//! that persist into a TEXT column should base64-encode the envelope and tag it
//! with a discriminator so cleartext-vs-ciphertext stays unambiguous (see
//! `xiaoguai-storage`).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use rand::Rng as _;
use thiserror::Error;

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
    /// A current key env var is missing AND the caller insists a keyring is
    /// required (a strict refuse-to-start contract). Opt-in callers use
    /// [`Keyring::from_env_vars`]'s `Ok(None)` instead.
    #[error("encryption key is required but not set")]
    KeyMissing,

    /// An env var was present but did not decode as a 32-byte base64url value.
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
///
/// `Debug` is redacted so the key bytes can never leak into logs via a
/// `{:?}`-formatted struct that happens to embed a [`Keyring`].
#[derive(Clone)]
pub struct AeadKey(pub [u8; KEY_LEN]);

impl std::fmt::Debug for AeadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AeadKey").field(&"[redacted; 32]").finish()
    }
}

impl AeadKey {
    /// Parse a base64url-encoded 32-byte key. Accepts both padded and
    /// unpadded forms.
    ///
    /// `env_name` is used only to attribute a [`AtRestError::KeyMalformed`]
    /// to the offending variable.
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
        // aes-gcm 0.11 (hybrid-array): a fixed-size array ref converts
        // infallibly; `from_slice` is deprecated.
        (&self.0).into()
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
    pub const fn with_keys(current: AeadKey, prev: Option<AeadKey>) -> Self {
        Self { current, prev }
    }

    /// Load a keyring from a caller-chosen pair of environment variables.
    ///
    /// Returns `Ok(None)` when `current_var` is unset or empty — the caller
    /// decides whether that means "encryption disabled" (opt-in field
    /// encryption) or an error (a strict refuse-to-start contract). Returns
    /// `Err` only when a variable is *present but malformed*, which is always a
    /// misconfiguration worth surfacing loudly rather than silently falling
    /// back to cleartext.
    ///
    /// # Errors
    /// [`AtRestError::KeyMalformed`] when either variable is present but does
    /// not decode to a 32-byte base64url value.
    pub fn from_env_vars(
        current_var: &'static str,
        prev_var: &'static str,
    ) -> Result<Option<Self>, AtRestError> {
        let current_raw = std::env::var(current_var)
            .ok()
            .filter(|s| !s.trim().is_empty());
        let prev_raw = std::env::var(prev_var)
            .ok()
            .filter(|s| !s.trim().is_empty());

        let Some(cur) = current_raw else {
            return Ok(None);
        };
        let current = AeadKey::from_b64url(&cur, current_var)?;
        let prev = match prev_raw {
            Some(s) => Some(AeadKey::from_b64url(&s, prev_var)?),
            None => None,
        };
        Ok(Some(Self { current, prev }))
    }

    /// Encrypt a plaintext secret. Returns the framed envelope
    /// `[version || nonce || ct]`.
    ///
    /// # Errors
    /// Returns [`AtRestError::Encrypt`] if the underlying AEAD call fails.
    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, AtRestError> {
        let cipher = Aes256Gcm::new(self.current.as_gcm_key());
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ct = cipher
            .encrypt(
                &nonce,
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
    /// - [`AtRestError::Envelope`] if the envelope is shorter than the header
    ///   or carries an unknown version byte.
    /// - [`AtRestError::Decrypt`] if all keys fail.
    /// - [`AtRestError::NotUtf8`] if decrypted bytes are not UTF-8.
    pub fn decrypt(&self, envelope: &[u8]) -> Result<String, AtRestError> {
        if envelope.len() < 1 + NONCE_LEN {
            return Err(AtRestError::Envelope("ciphertext shorter than header"));
        }
        if envelope[0] != ENVELOPE_VERSION {
            return Err(AtRestError::Envelope("unknown envelope version byte"));
        }
        // Length is guaranteed by the header check above, so the fixed-size
        // copy (and the infallible `From`) replaces the deprecated
        // `Nonce::from_slice`.
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(&envelope[1..=NONCE_LEN]);
        let nonce = Nonce::from(nonce_bytes);
        let ct = &envelope[1 + NONCE_LEN..];

        let candidates = std::iter::once(&self.current).chain(self.prev.as_ref());
        for key in candidates {
            let cipher = Aes256Gcm::new(key.as_gcm_key());
            if let Ok(pt) = cipher.decrypt(&nonce, Payload { msg: ct, aad: b"" }) {
                return String::from_utf8(pt).map_err(|_| AtRestError::NotUtf8);
            }
        }
        Err(AtRestError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in env-var name for error-attribution in tests; the primitive is
    /// domain-neutral, so any `&'static str` works here.
    const TEST_VAR: &str = "XIAOGUAI_AT_REST_KEY_TEST";

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
        assert!(matches!(
            kr_b.decrypt(&ct).unwrap_err(),
            AtRestError::Decrypt
        ));
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
        assert_eq!(AeadKey::from_b64url(&padded, TEST_VAR).unwrap().0, raw);
        assert_eq!(AeadKey::from_b64url(&unpadded, TEST_VAR).unwrap().0, raw);
    }

    #[test]
    fn from_b64url_rejects_wrong_length() {
        let short = URL_SAFE_NO_PAD.encode([0u8; 16]);
        assert!(matches!(
            AeadKey::from_b64url(&short, TEST_VAR).unwrap_err(),
            AtRestError::KeyMalformed { .. }
        ));
    }

    #[test]
    fn from_b64url_rejects_non_base64() {
        assert!(matches!(
            AeadKey::from_b64url("@@@not-base64@@@", TEST_VAR).unwrap_err(),
            AtRestError::KeyMalformed { .. }
        ));
    }

    #[test]
    fn from_env_vars_returns_none_when_unset() {
        // A variable name that is guaranteed unset in the test environment.
        let got = Keyring::from_env_vars(
            "XIAOGUAI_AT_REST_KEY_DEFINITELY_UNSET_9z7",
            "XIAOGUAI_AT_REST_KEY_DEFINITELY_UNSET_9z7_PREV",
        )
        .unwrap();
        assert!(got.is_none());
    }
}
