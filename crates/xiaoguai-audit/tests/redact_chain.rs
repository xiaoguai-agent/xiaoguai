//! The redactor must run *before* HMAC signing, so a redacted entry's signature
//! is computed over the redacted form and the chain still verifies. This guards
//! the core safety claim of `PgAuditSink::with_redactor` without needing a
//! database (the live append path is exercised by the PG integration tests).

use chrono::Utc;
use serde_json::json;
use xiaoguai_audit::{AuditEntry, ChainedAudit, Redactor, StoredEntry};

const ZERO: [u8; 32] = [0u8; 32];

fn sample_entry() -> AuditEntry {
    AuditEntry {
        ts: Utc::now(),
        tenant_id: "tenant-1".into(),
        actor: "user:dave@example.com".into(),
        action: "session.create".into(),
        resource: Some("/inbox/10.0.0.1".into()),
        details: json!({"note": "client 192.168.0.1 connected"}),
    }
}

#[test]
fn redacted_entry_signs_and_verifies() {
    let chain = ChainedAudit::new(b"test-signing-key".to_vec());

    // Redact, then sign the redacted form (the order the sink uses).
    let redacted = Redactor::new().redact(sample_entry());
    assert!(
        redacted.actor.contains("[redacted-email]"),
        "PII not scrubbed"
    );

    let hmac = chain.compute_hmac(&ZERO, &redacted).expect("compute hmac");
    let stored = StoredEntry {
        id: 1,
        entry: redacted,
        prev_hmac: ZERO.to_vec(),
        hmac,
    };

    // Verifies because signing happened over the redacted bytes that are stored.
    chain
        .verify_chain(&ZERO, std::slice::from_ref(&stored))
        .expect("redacted entry must verify");
}

#[test]
fn unredacted_tamper_breaks_the_chain() {
    let chain = ChainedAudit::new(b"test-signing-key".to_vec());

    let redacted = Redactor::new().redact(sample_entry());
    let hmac = chain.compute_hmac(&ZERO, &redacted).expect("compute hmac");
    let mut tampered = StoredEntry {
        id: 1,
        entry: redacted,
        prev_hmac: ZERO.to_vec(),
        hmac,
    };

    // Putting the original (un-redacted) PII back while keeping the redacted
    // HMAC must fail verification — proving the signature binds the redacted form.
    tampered.entry.actor = "user:dave@example.com".into();
    assert!(
        chain
            .verify_chain(&ZERO, std::slice::from_ref(&tampered))
            .is_err(),
        "tampering an entry back to un-redacted content must break the chain"
    );
}
