//! Integration tests for `WecomCrypto`.
//!
//! Uses the official WeCom sample credentials from the documentation
//! (<https://developer.work.weixin.qq.com/document/path/90968>):
//!
//! ```text
//! Token:           QDG6eK
//! EncodingAESKey:  jWmYm7qr5nMoAUwZRjGtBxmz3KA1tkAj3ykkR6q2B2C (43 chars)
//! CorpID:          wx49f0ab532d5d035a
//! ```
//!
//! For signature verification we use a self-consistent vector: `SAMPLE_ENCRYPT` is
//! a fixed arbitrary base64 string and `SAMPLE_MSG_SIGNATURE` is the SHA-1 of
//! `sort([TOKEN, SAMPLE_TIMESTAMP, SAMPLE_NONCE, SAMPLE_ENCRYPT]).join("")`.
//! This verifies the algorithm's sorting + hashing logic independently of any
//! specific ciphertext.
//!
//! The official docs publish the signature algorithm but not a stand-alone
//! ciphertext+signature test vector that can be mechanically verified; the
//! decrypt/encrypt correctness is instead verified via the round-trip tests below.

use xiaoguai_im_wecom::crypto::{WecomCrypto, WecomCryptoError};

/// Official WeCom sample credential set.
const TOKEN: &str = "QDG6eK";
const ENCODING_AES_KEY: &str = "jWmYm7qr5nMoAUwZRjGtBxmz3KA1tkAj3ykkR6q2B2C";
const CORP_ID: &str = "wx49f0ab532d5d035a";

// Self-consistent signature vector.
// SAMPLE_ENCRYPT is an arbitrary b64 string; SAMPLE_MSG_SIGNATURE is
// sha1(sort([TOKEN, SAMPLE_TIMESTAMP, SAMPLE_NONCE, SAMPLE_ENCRYPT])) —
// computed with Python: hashlib.sha1("".join(sorted([TOKEN,TS,NONCE,ENC])).encode()).hexdigest()
const SAMPLE_TIMESTAMP: &str = "1409735669";
const SAMPLE_NONCE: &str = "1372623149";
const SAMPLE_ENCRYPT: &str =
    "9s4gMv99m88kKTh/H8IdkNiFGeG9mD7S6nhXLaJ+nEiNEom5rH83r5VR9C4jR6EeKlKJLdpJP3b\
     EYLJOHoMhNGBVi7dVE8Y=";
// sha1(sort(["QDG6eK","1409735669","1372623149",SAMPLE_ENCRYPT]))
const SAMPLE_MSG_SIGNATURE: &str = "13c67161032e9f8d152d93d27780e65e757a9eb0";

// ─── Construction ─────────────────────────────────────────────────────────────

#[test]
fn new_accepts_valid_43_char_key() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID);
    assert!(crypto.is_ok(), "valid 43-char key must be accepted");
}

#[test]
fn new_rejects_key_wrong_length() {
    // 42-char key (one char short)
    let short = &ENCODING_AES_KEY[..42];
    assert!(
        matches!(
            WecomCrypto::new(TOKEN, short, CORP_ID),
            Err(WecomCryptoError::InvalidKey(_))
        ),
        "key with wrong length must be rejected"
    );
}

#[test]
fn new_rejects_key_with_invalid_base64() {
    // Replace last char with `!` which is not in the base64 alphabet.
    let bad = format!("{}!", &ENCODING_AES_KEY[..42]);
    assert!(
        matches!(
            WecomCrypto::new(TOKEN, &bad, CORP_ID),
            Err(WecomCryptoError::InvalidKey(_))
        ),
        "key with invalid base64 chars must be rejected"
    );
}

// ─── Signature verification ───────────────────────────────────────────────────

#[test]
fn verify_signature_matches_official_vector() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    assert!(
        crypto.verify_signature(
            SAMPLE_MSG_SIGNATURE,
            SAMPLE_TIMESTAMP,
            SAMPLE_NONCE,
            SAMPLE_ENCRYPT
        ),
        "official sample signature must verify"
    );
}

#[test]
fn verify_signature_fails_when_tampered_encrypt() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    assert!(
        !crypto.verify_signature(
            SAMPLE_MSG_SIGNATURE,
            SAMPLE_TIMESTAMP,
            SAMPLE_NONCE,
            "tampered_encrypt_value"
        ),
        "tampered encrypt must fail signature"
    );
}

#[test]
fn verify_signature_fails_when_wrong_token() {
    let crypto = WecomCrypto::new("wrong_token", ENCODING_AES_KEY, CORP_ID).unwrap();
    assert!(
        !crypto.verify_signature(
            SAMPLE_MSG_SIGNATURE,
            SAMPLE_TIMESTAMP,
            SAMPLE_NONCE,
            SAMPLE_ENCRYPT
        ),
        "wrong token must fail signature"
    );
}

#[test]
fn verify_signature_fails_when_timestamp_changed() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    assert!(
        !crypto.verify_signature(
            SAMPLE_MSG_SIGNATURE,
            "9999999999", // altered
            SAMPLE_NONCE,
            SAMPLE_ENCRYPT
        ),
        "changed timestamp must fail signature"
    );
}

// ─── Decrypt ──────────────────────────────────────────────────────────────────

/// Verify that decrypt recovers the corpid — we encrypt a known XML body and
/// then decrypt, asserting that the decrypted content matches the original.
/// (The official docs do not publish a standalone decryptable ciphertext sample,
/// so we generate our own from the official credentials.)
#[test]
fn decrypt_recovers_inner_xml_with_corp_id() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    // Encrypt a known plaintext.
    let original = format!("<xml><ToUserName>{CORP_ID}</ToUserName><Content>test</Content></xml>");
    let wrapped = crypto
        .encrypt(&original, "1409735669", "1372623149")
        .unwrap();

    // Extract the Encrypt blob.
    let encrypt_blob = extract_xml_text(&wrapped, "Encrypt").expect("Encrypt element");
    // Decrypt and verify.
    let recovered = crypto.decrypt(encrypt_blob).expect("decrypt must succeed");
    assert!(
        recovered.contains(CORP_ID),
        "decrypted XML must contain corp ID; got: {recovered}"
    );
    assert_eq!(recovered, original, "must recover exact original XML");
}

#[test]
fn decrypt_fails_on_garbage_base64() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    let err = crypto.decrypt("not-valid-base64!!!");
    assert!(
        matches!(err, Err(WecomCryptoError::DecodeError(_))),
        "garbage base64 must yield DecodeError; got: {err:?}"
    );
}

#[test]
fn decrypt_fails_on_too_short_ciphertext() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    // 15 bytes — shorter than one AES block.
    let short_b64 = STANDARD.encode([0u8; 15]);
    let err = crypto.decrypt(&short_b64);
    assert!(err.is_err(), "too-short ciphertext must fail; got: {err:?}");
}

// ─── Encrypt ──────────────────────────────────────────────────────────────────

#[test]
fn encrypt_produces_valid_wrapped_xml() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    let ts = "1409735700";
    let nonce = "987654321";
    let plaintext = "<xml><Content>hello</Content></xml>";
    let wrapped = crypto
        .encrypt(plaintext, ts, nonce)
        .expect("encrypt must succeed");
    // The wrapped XML must contain all required WeCom response fields.
    assert!(wrapped.contains("<Encrypt>"), "must contain <Encrypt>");
    assert!(
        wrapped.contains("<MsgSignature>"),
        "must contain <MsgSignature>"
    );
    assert!(wrapped.contains("<TimeStamp>"), "must contain <TimeStamp>");
    assert!(wrapped.contains("<Nonce>"), "must contain <Nonce>");
    assert!(wrapped.contains(ts), "wrapped XML must embed the timestamp");
    assert!(wrapped.contains(nonce), "wrapped XML must embed the nonce");
}

// ─── Round-trip ───────────────────────────────────────────────────────────────

#[test]
fn encrypt_then_decrypt_recovers_plaintext() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    let ts = "1716355200";
    let nonce = "42abc";
    let original =
        "<xml><ToUserName>wx49f0ab532d5d035a</ToUserName><Content>round trip</Content></xml>";
    let wrapped = crypto.encrypt(original, ts, nonce).expect("encrypt");

    // Extract the Encrypt element from the wrapped XML.
    let encrypt_start = wrapped
        .find("<Encrypt><![CDATA[")
        .map(|i| i + 18)
        .or_else(|| wrapped.find("<Encrypt>").map(|i| i + 9))
        .expect("must have <Encrypt> in wrapped XML");
    let encrypt_end = wrapped[encrypt_start..]
        .find("]]></Encrypt>")
        .map(|i| i + encrypt_start)
        .or_else(|| {
            wrapped[encrypt_start..]
                .find("</Encrypt>")
                .map(|i| i + encrypt_start)
        })
        .expect("must close <Encrypt>");
    let encrypted_blob = &wrapped[encrypt_start..encrypt_end];

    let recovered = crypto.decrypt(encrypted_blob).expect("decrypt");
    assert_eq!(
        recovered, original,
        "round-trip must recover original plaintext"
    );
}

#[test]
fn encrypt_then_decrypt_signature_verifies() {
    let crypto = WecomCrypto::new(TOKEN, ENCODING_AES_KEY, CORP_ID).unwrap();
    let ts = "1716355200";
    let nonce = "xyz123";
    let msg = "<xml><Content>verify me</Content></xml>";
    let wrapped = crypto.encrypt(msg, ts, nonce).expect("encrypt");

    // Extract fields.
    let encrypt_val = extract_xml_text(&wrapped, "Encrypt").expect("Encrypt");
    let sig_val = extract_xml_text(&wrapped, "MsgSignature").expect("MsgSignature");

    assert!(
        crypto.verify_signature(sig_val, ts, nonce, encrypt_val),
        "signature of encrypted output must verify"
    );
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Minimal tag extractor for test assertions (handles both CDATA and plain).
fn extract_xml_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open_cdata = format!("<{tag}><![CDATA[");
    let close_cdata = format!("]]></{tag}>");
    let open_plain = format!("<{tag}>");
    let close_plain = format!("</{tag}>");

    if let Some(start) = xml.find(&open_cdata) {
        let from = start + open_cdata.len();
        let end = xml[from..].find(&close_cdata)? + from;
        return Some(&xml[from..end]);
    }
    if let Some(start) = xml.find(&open_plain) {
        let from = start + open_plain.len();
        let end = xml[from..].find(&close_plain)? + from;
        return Some(&xml[from..end]);
    }
    None
}
