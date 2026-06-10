//! Ed25519 signature verification for Discord Interactions webhooks.
//!
//! Discord authenticates every webhook request with two headers:
//!
//! - `X-Signature-Ed25519` — hex-encoded Ed25519 signature over the
//!   concatenation of `X-Signature-Timestamp` + raw body bytes.
//! - `X-Signature-Timestamp` — UNIX-epoch second string. SEC-05/SEC-12:
//!   [`verify`] enforces a ±[`TIMESTAMP_TOLERANCE_SECS`] freshness window
//!   against the caller-supplied `now_unix` so captured requests cannot be
//!   replayed after the window closes.
//!
//! Verification steps (per Discord docs):
//! ```text
//! message = timestamp_bytes || body_bytes
//! verify(public_key, message, hex_decode(signature_header))
//! ```

use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use xiaoguai_im_gateway::ProviderError;

/// SEC-05/SEC-12: maximum clock skew (seconds) we allow between
/// `X-Signature-Timestamp` and the current wall clock — the replay window.
/// Mirrors the Slack adapter's 5-minute tolerance.
pub const TIMESTAMP_TOLERANCE_SECS: i64 = 300;

/// Current Unix time in seconds. Falls back to 0 when the system clock
/// reports a pre-epoch time, which pushes every inbound timestamp outside
/// the replay window (fail-closed).
pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Verify a Discord webhook request.
///
/// `now_unix` is the current Unix timestamp (seconds). Pass the
/// crate-private `now_unix()` helper in production; pass a fixed value
/// in tests.
///
/// # Errors
/// Returns [`ProviderError::BadSignature`] when:
/// - either header is absent,
/// - the timestamp is not a decimal integer or falls outside
///   ±[`TIMESTAMP_TOLERANCE_SECS`] of `now_unix` (SEC-05 replay window),
/// - the signature header is not valid lowercase hex,
/// - the signature is 64 bytes but fails Ed25519 verification,
/// - `public_key` is not a valid 32-byte compressed point.
pub fn verify(
    public_key: &VerifyingKey,
    timestamp: &str,
    body: &[u8],
    signature_hex: &str,
    now_unix: i64,
) -> Result<(), ProviderError> {
    // SEC-05/SEC-12 replay protection: reject stale/future timestamps
    // before any cryptographic work (fail-closed).
    let ts: i64 = timestamp.parse().map_err(|_| ProviderError::BadSignature)?;
    if (ts - now_unix).abs() > TIMESTAMP_TOLERANCE_SECS {
        return Err(ProviderError::BadSignature);
    }

    let sig_bytes = hex::decode(signature_hex).map_err(|_| ProviderError::BadSignature)?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|_| ProviderError::BadSignature)?;

    // message = timestamp || body
    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    public_key
        .verify(&message, &signature)
        .map_err(|_| ProviderError::BadSignature)
}

/// Parse the Discord application public key from its hex representation.
///
/// # Errors
/// Returns [`ProviderError::Transport`] when the string is not 64 hex chars
/// representing a valid Ed25519 compressed point.
pub fn parse_public_key(hex_key: &str) -> Result<VerifyingKey, ProviderError> {
    let key_bytes = hex::decode(hex_key)
        .map_err(|e| ProviderError::Transport(format!("decode public key hex: {e}")))?;
    let arr: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| ProviderError::Transport("public key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| ProviderError::Transport(format!("invalid Ed25519 public key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};
    use rand::Rng;

    fn make_keypair() -> (SigningKey, VerifyingKey) {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    fn sign(signing_key: &SigningKey, timestamp: &str, body: &[u8]) -> String {
        let mut message = Vec::new();
        message.extend_from_slice(timestamp.as_bytes());
        message.extend_from_slice(body);
        let sig = signing_key.sign(&message);
        hex::encode(sig.to_bytes())
    }

    const NOW: i64 = 1_716_355_200;

    #[test]
    fn valid_signature_passes() {
        let (sk, vk) = make_keypair();
        let ts = "1716355200";
        let body = b"{}";
        let sig_hex = sign(&sk, ts, body);
        verify(&vk, ts, body, &sig_hex, NOW).expect("should pass");
    }

    #[test]
    fn wrong_body_fails() {
        let (sk, vk) = make_keypair();
        let ts = "1716355200";
        let sig_hex = sign(&sk, ts, b"original body");
        assert!(matches!(
            verify(&vk, ts, b"tampered body", &sig_hex, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn wrong_timestamp_fails() {
        let (sk, vk) = make_keypair();
        let body = b"{}";
        let sig_hex = sign(&sk, "1716355200", body);
        // "1716355201" is inside the freshness window, so the failure
        // exercised here is the Ed25519 mismatch, not the replay check.
        assert!(matches!(
            verify(&vk, "1716355201", body, &sig_hex, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn invalid_hex_signature_fails() {
        let (_, vk) = make_keypair();
        assert!(matches!(
            verify(&vk, "1716355200", b"body", "not-hex", NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn non_numeric_timestamp_fails() {
        let (sk, vk) = make_keypair();
        let body = b"{}";
        let sig_hex = sign(&sk, "ts", body);
        assert!(matches!(
            verify(&vk, "ts", body, &sig_hex, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    /// SEC-05: a correctly signed request whose timestamp is older than
    /// the tolerance window must be rejected (replay).
    #[test]
    fn expired_timestamp_rejected() {
        let (sk, vk) = make_keypair();
        let ts = (NOW - TIMESTAMP_TOLERANCE_SECS - 1).to_string();
        let body = b"{}";
        let sig_hex = sign(&sk, &ts, body);
        assert!(matches!(
            verify(&vk, &ts, body, &sig_hex, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    /// SEC-05: a timestamp exactly at the tolerance boundary still passes.
    #[test]
    fn timestamp_within_window_accepted() {
        let (sk, vk) = make_keypair();
        let ts = (NOW - TIMESTAMP_TOLERANCE_SECS).to_string();
        let body = b"{}";
        let sig_hex = sign(&sk, &ts, body);
        verify(&vk, &ts, body, &sig_hex, NOW).expect("within window");
    }

    #[test]
    fn signature_from_different_key_fails() {
        let (sk1, _vk1) = make_keypair();
        let (_sk2, vk2) = make_keypair();
        let ts = "1716355200";
        let body = b"{}";
        let sig_hex = sign(&sk1, ts, body);
        assert!(matches!(
            verify(&vk2, ts, body, &sig_hex, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn parse_public_key_roundtrip() {
        let (_, vk) = make_keypair();
        let hex_key = hex::encode(vk.as_bytes());
        let parsed = parse_public_key(&hex_key).expect("valid key");
        assert_eq!(parsed.as_bytes(), vk.as_bytes());
    }

    #[test]
    fn parse_public_key_bad_hex_errors() {
        assert!(parse_public_key("not-hex").is_err());
    }

    #[test]
    fn parse_public_key_wrong_length_errors() {
        // 31 bytes → wrong length
        let short = hex::encode([0u8; 31]);
        assert!(parse_public_key(&short).is_err());
    }
}
