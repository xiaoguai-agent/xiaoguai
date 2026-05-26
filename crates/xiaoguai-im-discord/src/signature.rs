//! Ed25519 signature verification for Discord Interactions webhooks.
//!
//! Discord authenticates every webhook request with two headers:
//!
//! - `X-Signature-Ed25519` — hex-encoded Ed25519 signature over the
//!   concatenation of `X-Signature-Timestamp` + raw body bytes.
//! - `X-Signature-Timestamp` — UNIX-epoch second string; Discord rejects
//!   requests it delivered more than a few seconds ago, but verification
//!   here is stateless — callers that need replay protection must check
//!   the timestamp themselves.
//!
//! Verification steps (per Discord docs):
//! ```text
//! message = timestamp_bytes || body_bytes
//! verify(public_key, message, hex_decode(signature_header))
//! ```

use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use xiaoguai_im_gateway::ProviderError;

/// Verify a Discord webhook request.
///
/// # Errors
/// Returns [`ProviderError::BadSignature`] when:
/// - either header is absent,
/// - the signature header is not valid lowercase hex,
/// - the signature is 64 bytes but fails Ed25519 verification,
/// - `public_key` is not a valid 32-byte compressed point.
pub fn verify(
    public_key: &VerifyingKey,
    timestamp: &str,
    body: &[u8],
    signature_hex: &str,
) -> Result<(), ProviderError> {
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

    #[test]
    fn valid_signature_passes() {
        let (sk, vk) = make_keypair();
        let ts = "1716355200";
        let body = b"{}";
        let sig_hex = sign(&sk, ts, body);
        verify(&vk, ts, body, &sig_hex).expect("should pass");
    }

    #[test]
    fn wrong_body_fails() {
        let (sk, vk) = make_keypair();
        let ts = "1716355200";
        let sig_hex = sign(&sk, ts, b"original body");
        assert!(matches!(
            verify(&vk, ts, b"tampered body", &sig_hex),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn wrong_timestamp_fails() {
        let (sk, vk) = make_keypair();
        let body = b"{}";
        let sig_hex = sign(&sk, "1716355200", body);
        assert!(matches!(
            verify(&vk, "9999999999", body, &sig_hex),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn invalid_hex_signature_fails() {
        let (_, vk) = make_keypair();
        assert!(matches!(
            verify(&vk, "ts", b"body", "not-hex"),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn signature_from_different_key_fails() {
        let (sk1, _vk1) = make_keypair();
        let (_sk2, vk2) = make_keypair();
        let ts = "1716355200";
        let body = b"{}";
        let sig_hex = sign(&sk1, ts, body);
        assert!(matches!(
            verify(&vk2, ts, body, &sig_hex),
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
