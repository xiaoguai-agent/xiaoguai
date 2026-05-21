//! Unit tests for the pure HMAC chain logic. No Postgres required.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use xiaoguai_audit::chain::{AuditEntry, ChainError, ChainedAudit, StoredEntry, HMAC_LEN};

const KEY: &[u8] = b"test-key-do-not-use-in-prod";

fn entry(action: &str, details: serde_json::Value) -> AuditEntry {
    AuditEntry {
        ts: Utc.with_ymd_and_hms(2026, 5, 20, 10, 0, 0).unwrap(),
        tenant_id: "tenant-1".into(),
        actor: "user:1".into(),
        action: action.into(),
        resource: Some("session:abc".into()),
        details,
    }
}

fn build_chain(chain: &ChainedAudit, entries: Vec<AuditEntry>) -> Vec<StoredEntry> {
    let mut prev = vec![0u8; HMAC_LEN];
    let mut stored = Vec::with_capacity(entries.len());
    for (i, e) in entries.into_iter().enumerate() {
        let h = chain.compute_hmac(&prev, &e).expect("hmac");
        stored.push(StoredEntry {
            id: i64::try_from(i + 1).unwrap(),
            entry: e,
            prev_hmac: prev.clone(),
            hmac: h.clone(),
        });
        prev = h;
    }
    stored
}

#[test]
fn compute_hmac_is_deterministic() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let e = entry("session.create", json!({ "x": 1 }));
    let prev = vec![0u8; HMAC_LEN];
    let a = chain.compute_hmac(&prev, &e).unwrap();
    let b = chain.compute_hmac(&prev, &e).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), HMAC_LEN);
}

#[test]
fn different_prev_hmac_produces_different_output() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let e = entry("session.create", json!({ "x": 1 }));
    let p1 = vec![0u8; HMAC_LEN];
    let mut p2 = vec![0u8; HMAC_LEN];
    p2[0] = 1;
    let a = chain.compute_hmac(&p1, &e).unwrap();
    let b = chain.compute_hmac(&p2, &e).unwrap();
    assert_ne!(a, b);
}

#[test]
fn different_entry_produces_different_output() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let e1 = entry("session.create", json!({ "x": 1 }));
    let e2 = entry("session.create", json!({ "x": 2 }));
    let prev = vec![0u8; HMAC_LEN];
    let a = chain.compute_hmac(&prev, &e1).unwrap();
    let b = chain.compute_hmac(&prev, &e2).unwrap();
    assert_ne!(a, b);
}

#[test]
fn json_key_order_does_not_matter() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let prev = vec![0u8; HMAC_LEN];
    let mut e1 = entry("x", json!({ "a": 1, "b": 2, "c": { "d": 3, "e": 4 } }));
    let mut e2 = entry("x", json!({ "c": { "e": 4, "d": 3 }, "b": 2, "a": 1 }));
    // ensure timestamps identical
    let ts: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 5, 20, 10, 0, 0).unwrap();
    e1.ts = ts;
    e2.ts = ts;
    let a = chain.compute_hmac(&prev, &e1).unwrap();
    let b = chain.compute_hmac(&prev, &e2).unwrap();
    assert_eq!(a, b, "canonical encoding must normalize JSON key order");
}

#[test]
fn verify_chain_accepts_valid_sequence() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let entries = vec![
        entry("session.create", json!({ "n": 1 })),
        entry("tool.invoke", json!({ "n": 2 })),
        entry("cost.charge", json!({ "n": 3 })),
    ];
    let stored = build_chain(&chain, entries);
    let zero = [0u8; HMAC_LEN];
    chain.verify_chain(&zero, &stored).expect("valid chain");
}

#[test]
fn verify_chain_rejects_tampered_details() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let entries = vec![
        entry("session.create", json!({ "n": 1 })),
        entry("tool.invoke", json!({ "n": 2 })),
    ];
    let mut stored = build_chain(&chain, entries);
    // Flip a byte by mutating the JSON payload of the second entry.
    stored[1].entry.details = json!({ "n": 999 });
    let zero = [0u8; HMAC_LEN];
    let err = chain.verify_chain(&zero, &stored).unwrap_err();
    assert!(
        matches!(err, ChainError::HmacMismatch(_)),
        "expected hmac mismatch, got {err:?}"
    );
}

#[test]
fn verify_chain_rejects_broken_link() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let entries = vec![
        entry("a", json!({})),
        entry("b", json!({})),
        entry("c", json!({})),
    ];
    let stored = build_chain(&chain, entries);
    // Skip the middle entry — entries[2].prev_hmac no longer matches entries[0].hmac.
    let truncated = vec![stored[0].clone(), stored[2].clone()];
    let zero = [0u8; HMAC_LEN];
    let err = chain.verify_chain(&zero, &truncated).unwrap_err();
    assert!(
        matches!(err, ChainError::LinkBroken(_, _)),
        "expected link broken, got {err:?}"
    );
}

#[test]
fn verify_chain_rejects_wrong_starting_prev() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let entries = vec![entry("a", json!({}))];
    let stored = build_chain(&chain, entries);
    let mut bad_start = vec![0u8; HMAC_LEN];
    bad_start[0] = 7;
    let err = chain.verify_chain(&bad_start, &stored).unwrap_err();
    assert!(matches!(err, ChainError::LinkBroken(_, _)));
}

#[test]
fn empty_chain_is_trivially_valid() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let zero = [0u8; HMAC_LEN];
    chain.verify_chain(&zero, &[]).expect("empty chain ok");
}

#[test]
fn invalid_prev_hmac_length_is_rejected() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let e = entry("a", json!({}));
    let short = vec![0u8; 16];
    let err = chain.compute_hmac(&short, &e).unwrap_err();
    assert!(matches!(err, ChainError::InvalidHmacLength));
}
