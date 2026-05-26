//! WeCom `EncodingAESKey` — AES-256-CBC encrypt / decrypt.
//!
//! ## Algorithm (WeCom official spec)
//!
//! ### Key derivation
//! The 43-character `EncodingAESKey` is standard base64 **without** the
//! trailing `=`.  Append `=` to make 44 chars, then base64-decode to get
//! 32 bytes = AES-256 key.  The IV is the **first 16 bytes** of that key.
//!
//! ### Inbound signature verification
//! ```text
//! sha1(sort([token, timestamp, nonce, encrypt]).concat(""))
//! ```
//! (lexicographic sort, plain concatenation, lower-case hex digest)
//!
//! ### Decryption
//! 1. Base64-decode the `Encrypt` field.
//! 2. AES-256-CBC decrypt with `key[0..32]` and `iv = key[0..16]`.
//! 3. Strip PKCS#7 padding: the last `pad_len` bytes are all `pad_len`.
//! 4. The plaintext layout:
//!    ```text
//!    [ 16 random bytes ][ 4-byte big-endian msg_len ][ msg bytes ][ corpid bytes ]
//!    ```
//!    Validate that `corpid` at the end matches the configured corp ID.
//!
//! ### Encryption (reply path)
//! 1. Build the plaintext buffer: `random16 + BE32(len) + msg + corpid`.
//! 2. Pad with PKCS#7 (block size 32 — WeCom-specific).
//! 3. AES-256-CBC encrypt.
//! 4. Base64-encode → `Encrypt` element.
//! 5. Compute `MsgSignature = sha1(sort([token, ts, nonce, encrypt]))`.
//! 6. Wrap in:
//!    ```xml
//!    <xml>
//!      <Encrypt><![CDATA[...]]></Encrypt>
//!      <MsgSignature><![CDATA[...]]></MsgSignature>
//!      <TimeStamp>...</TimeStamp>
//!      <Nonce><![CDATA[...]]></Nonce>
//!    </xml>
//!    ```

use aes::Aes256;
use base64::{
    alphabet,
    engine::{general_purpose::STANDARD as B64, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use cbc::{Decryptor, Encryptor};
use cipher::{block_padding::NoPadding, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use rand::Rng;
use sha1::{Digest, Sha1};

/// A permissive base64 engine that ignores non-zero padding bits (as
/// Python's `b64decode` and the WeCom SDK do).  WeCom's 43-char
/// `EncodingAESKey` can encode a 32-byte key whose last two base64
/// chars have non-zero trailing bits that a strict decoder rejects.
fn wecom_b64() -> GeneralPurpose {
    GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    )
}

/// AES block / WeCom pad-block size (bytes).
const BLOCK: usize = 32;

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum WecomCryptoError {
    #[error("invalid EncodingAESKey: {0}")]
    InvalidKey(String),
    #[error("decrypt failed: {0}")]
    DecryptFailed(String),
    #[error("base64 decode error: {0}")]
    DecodeError(String),
    #[error("corpid mismatch in decrypted payload")]
    CorpIdMismatch,
}

// ─── Public struct ────────────────────────────────────────────────────────────

/// Handles WeCom AES-256-CBC encryption / decryption and SHA-1 signature
/// verification for encrypted callback payloads.
pub struct WecomCrypto {
    token: String,
    aes_key: [u8; 32],
    corpid: String,
}

impl WecomCrypto {
    /// Construct from the three WeCom console credentials.
    ///
    /// `encoding_aes_key` must be exactly 43 base64-alphabet characters (the
    /// WeCom console omits the trailing `=`).  Returns
    /// [`WecomCryptoError::InvalidKey`] if the key has the wrong length or
    /// contains non-base64 characters.
    ///
    /// # Errors
    /// Returns `WecomCryptoError::InvalidKey` if `encoding_aes_key` is not
    /// exactly 43 characters, not valid base64, or does not decode to 32 bytes.
    pub fn new(
        token: impl Into<String>,
        encoding_aes_key: &str,
        corpid: impl Into<String>,
    ) -> Result<Self, WecomCryptoError> {
        if encoding_aes_key.len() != 43 {
            return Err(WecomCryptoError::InvalidKey(format!(
                "expected 43 chars, got {}",
                encoding_aes_key.len()
            )));
        }
        // Append the missing `=` padding to make valid base64.  Use a
        // permissive engine because WeCom keys can have non-zero trailing
        // bits that a strict STANDARD decoder rejects.
        let padded = format!("{encoding_aes_key}=");
        let raw = wecom_b64()
            .decode(padded.as_bytes())
            .map_err(|e| WecomCryptoError::InvalidKey(format!("base64 decode: {e}")))?;
        if raw.len() != 32 {
            return Err(WecomCryptoError::InvalidKey(format!(
                "decoded to {} bytes, expected 32",
                raw.len()
            )));
        }
        let mut aes_key = [0u8; 32];
        aes_key.copy_from_slice(&raw);
        Ok(Self {
            token: token.into(),
            aes_key,
            corpid: corpid.into(),
        })
    }

    /// Verify the WeCom request signature.
    ///
    /// WeCom computes:
    /// ```text
    /// sha1(sort([token, timestamp, nonce, encrypt]).concat())
    /// ```
    /// and places the lower-case hex result in `msg_signature`.
    ///
    /// Returns `true` when the computed digest matches `signature` in
    /// constant time.
    #[must_use]
    pub fn verify_signature(
        &self,
        signature: &str,
        timestamp: &str,
        nonce: &str,
        encrypt: &str,
    ) -> bool {
        let computed = sha1_wecom(&self.token, timestamp, nonce, encrypt);
        constant_time_eq(computed.as_bytes(), signature.as_bytes())
    }

    /// Decrypt the base64-encoded `Encrypt` field from an inbound WeCom
    /// callback.
    ///
    /// On success returns the inner XML as a `String`.
    ///
    /// # Errors
    /// Returns `WecomCryptoError` if the base64 is invalid, AES decryption
    /// fails, PKCS#7 padding is invalid, or the corp-id tail does not match.
    ///
    /// # Panics
    /// Panics if the sliced byte ranges are out of bounds — this is a logic
    /// guard and should not occur when the ciphertext comes from WeCom.
    pub fn decrypt(&self, encrypt: &str) -> Result<String, WecomCryptoError> {
        let cipher_bytes = wecom_b64()
            .decode(encrypt)
            .map_err(|e| WecomCryptoError::DecodeError(format!("base64: {e}")))?;

        // AES-256-CBC decrypt; IV = first 16 bytes of the key.
        let iv: [u8; 16] = self.aes_key[..16].try_into().expect("16 bytes");
        let decrypted = aes256_cbc_decrypt(&self.aes_key, &iv, &cipher_bytes)
            .map_err(WecomCryptoError::DecryptFailed)?;

        // Strip WeCom PKCS#7 padding (pad_len == last byte, block size 32).
        let plaintext = pkcs7_unpad(&decrypted, BLOCK)
            .ok_or_else(|| WecomCryptoError::DecryptFailed("invalid pkcs7 padding".into()))?;

        // Layout: [ 16 random ] [ 4-byte BE msg_len ] [ msg ] [ corpid ]
        if plaintext.len() < 20 {
            return Err(WecomCryptoError::DecryptFailed(
                "decrypted content too short".into(),
            ));
        }
        let msg_len =
            u32::from_be_bytes(plaintext[16..20].try_into().expect("exactly 4 bytes")) as usize;
        let total_needed = 20 + msg_len + self.corpid.len();
        if plaintext.len() < total_needed {
            return Err(WecomCryptoError::DecryptFailed(format!(
                "payload too short: need {total_needed}, have {}",
                plaintext.len()
            )));
        }
        let msg_bytes = &plaintext[20..20 + msg_len];
        let tail = &plaintext[20 + msg_len..20 + msg_len + self.corpid.len()];
        if tail != self.corpid.as_bytes() {
            return Err(WecomCryptoError::CorpIdMismatch);
        }
        String::from_utf8(msg_bytes.to_vec())
            .map_err(|e| WecomCryptoError::DecryptFailed(format!("utf8: {e}")))
    }

    /// Encrypt a reply XML string and return the WeCom-format wrapped XML.
    ///
    /// The returned string looks like:
    /// ```xml
    /// <xml>
    ///   <Encrypt><![CDATA[...]]></Encrypt>
    ///   <MsgSignature><![CDATA[...]]></MsgSignature>
    ///   <TimeStamp>...</TimeStamp>
    ///   <Nonce><![CDATA[...]]></Nonce>
    /// </xml>
    /// ```
    ///
    /// # Errors
    /// Returns `WecomCryptoError` if the message is too large for the WeCom
    /// protocol or if AES encryption fails.
    ///
    /// # Panics
    /// Panics if the random-bytes fill fails — this is an OS-level RNG call
    /// and should not fail in production.
    pub fn encrypt(
        &self,
        msg: &str,
        timestamp: &str,
        nonce: &str,
    ) -> Result<String, WecomCryptoError> {
        // Build plaintext: random16 + BE32(len) + msg + corpid.
        let mut rng = rand::rng();
        let mut random16 = [0u8; 16];
        rng.fill_bytes(&mut random16);

        let msg_bytes = msg.as_bytes();
        let corpid_bytes = self.corpid.as_bytes();
        let msg_len = u32::try_from(msg_bytes.len()).map_err(|_| {
            WecomCryptoError::DecryptFailed("message too large for WeCom protocol".into())
        })?;

        let mut plaintext = Vec::with_capacity(16 + 4 + msg_bytes.len() + corpid_bytes.len());
        plaintext.extend_from_slice(&random16);
        plaintext.extend_from_slice(&msg_len.to_be_bytes());
        plaintext.extend_from_slice(msg_bytes);
        plaintext.extend_from_slice(corpid_bytes);

        // PKCS#7 pad to BLOCK (32).
        let padded = pkcs7_pad(plaintext, BLOCK);

        // AES-256-CBC encrypt; IV = first 16 bytes of key.
        let iv: [u8; 16] = self.aes_key[..16].try_into().expect("16 bytes");
        let cipher_bytes = aes256_cbc_encrypt(&self.aes_key, &iv, &padded)
            .map_err(WecomCryptoError::DecryptFailed)?;

        let encrypt_b64 = B64.encode(&cipher_bytes);

        // Compute MsgSignature.
        let sig = sha1_wecom(&self.token, timestamp, nonce, &encrypt_b64);

        // Build wrapped XML.
        let wrapped = format!(
            "<xml>\
            <Encrypt><![CDATA[{encrypt_b64}]]></Encrypt>\
            <MsgSignature><![CDATA[{sig}]]></MsgSignature>\
            <TimeStamp>{timestamp}</TimeStamp>\
            <Nonce><![CDATA[{nonce}]]></Nonce>\
            </xml>"
        );
        Ok(wrapped)
    }
}

// ─── Cryptographic helpers ────────────────────────────────────────────────────

fn sha1_wecom(token: &str, timestamp: &str, nonce: &str, encrypt: &str) -> String {
    let mut parts = [token, timestamp, nonce, encrypt];
    parts.sort_unstable();
    let joined: String = parts.concat();
    let mut h = Sha1::new();
    h.update(joined.as_bytes());
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Constant-time byte-slice equality.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// PKCS#7 pad `data` to a multiple of `block_size`.
fn pkcs7_pad(mut data: Vec<u8>, block_size: usize) -> Vec<u8> {
    let remainder = data.len() % block_size;
    let pad_len = if remainder == 0 {
        block_size
    } else {
        block_size - remainder
    };
    // pad_len is in range [1, block_size], which fits in u8 for block_size ≤ 255.
    #[allow(clippy::cast_possible_truncation)]
    let pad_byte = pad_len as u8;
    data.resize(data.len() + pad_len, pad_byte);
    data
}

/// PKCS#7 unpad.  Returns `None` if the padding bytes are inconsistent.
fn pkcs7_unpad(data: &[u8], block_size: usize) -> Option<Vec<u8>> {
    if data.is_empty() {
        return None;
    }
    let pad_len = *data.last()? as usize;
    if pad_len == 0 || pad_len > block_size || pad_len > data.len() {
        return None;
    }
    // Verify all padding bytes equal pad_len.
    for &b in &data[data.len() - pad_len..] {
        if b as usize != pad_len {
            return None;
        }
    }
    Some(data[..data.len() - pad_len].to_vec())
}

type Aes256CbcDec = Decryptor<Aes256>;
type Aes256CbcEnc = Encryptor<Aes256>;

fn aes256_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() % 16 != 0 || data.is_empty() {
        return Err(format!(
            "ciphertext length {} is not a multiple of 16",
            data.len()
        ));
    }
    let decryptor =
        Aes256CbcDec::new_from_slices(key, iv).map_err(|e| format!("init decryptor: {e}"))?;

    // cbc crate's decrypt_padded_vec expects NoPadding / Pkcs7 — we use
    // NoPadding because WeCom uses a custom block size (32) for padding, which
    // we handle ourselves.
    decryptor
        .decrypt_padded_vec::<NoPadding>(data)
        .map_err(|e| format!("cbc decrypt: {e}"))
}

fn aes256_cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() % 16 != 0 || data.is_empty() {
        return Err(format!(
            "plaintext length {} is not a multiple of 16",
            data.len()
        ));
    }
    let encryptor =
        Aes256CbcEnc::new_from_slices(key, iv).map_err(|e| format!("init encryptor: {e}"))?;

    Ok(encryptor.encrypt_padded_vec::<NoPadding>(data))
}
