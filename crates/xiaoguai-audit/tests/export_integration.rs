//! End-to-end test of the compliance export pipeline.
//!
//! No Postgres required — uses the same in-memory `build_chain` helper
//! pattern as `tests/chain_basic.rs`. Mirrors the in-memory storage approach
//! that PR #72 (`skill_author`) used for its E2E test.

use chrono::{TimeZone, Utc};
use serde_json::json;
use xiaoguai_audit::chain::HMAC_LEN;
use xiaoguai_audit::{
    export_bundle, render_csv, render_json, AuditEntry, ChainedAudit, ExportError, ExportWindow,
    Format, Framework, StoredEntry,
};

const KEY: &[u8] = b"export-integration-test-key";

fn build_chain(chain: &ChainedAudit, entries: Vec<AuditEntry>) -> Vec<StoredEntry> {
    let mut prev = vec![0u8; HMAC_LEN];
    let mut out = Vec::with_capacity(entries.len());
    for (i, e) in entries.into_iter().enumerate() {
        let h = chain.compute_hmac(&prev, &e).expect("hmac compute");
        out.push(StoredEntry {
            id: i64::try_from(i + 1).unwrap(),
            entry: e,
            prev_hmac: prev.clone(),
            hmac: h.clone(),
        });
        prev = h;
    }
    out
}

fn synthetic_entries() -> Vec<AuditEntry> {
    let base = Utc.with_ymd_and_hms(2026, 5, 10, 9, 0, 0).unwrap();
    vec![
        AuditEntry {
            ts: base,
            tenant_id: "t-acme".into(),
            actor: "user:42".into(),
            action: "session.create".into(),
            resource: Some("session:xyz".into()),
            details: json!({"client":"web"}),
        },
        AuditEntry {
            ts: base + chrono::Duration::seconds(30),
            tenant_id: "t-acme".into(),
            actor: "user:42".into(),
            action: "tool.invoke".into(),
            resource: Some("phi:patient/7".into()),
            details: json!({"tool":"chart-lookup"}),
        },
        AuditEntry {
            ts: base + chrono::Duration::minutes(1),
            tenant_id: "t-acme".into(),
            actor: "user:42".into(),
            action: "memory.recall".into(),
            resource: None,
            details: json!({"q":"history"}),
        },
        AuditEntry {
            ts: base + chrono::Duration::minutes(2),
            tenant_id: "t-acme".into(),
            actor: "system".into(),
            action: "policy.deny".into(),
            resource: Some("budget:llm".into()),
            details: json!({"reason":"cap"}),
        },
        AuditEntry {
            ts: base + chrono::Duration::minutes(3),
            tenant_id: "t-acme".into(),
            actor: "system".into(),
            action: "audit.verify".into(),
            resource: None,
            details: json!({"ok":true}),
        },
    ]
}

fn window() -> ExportWindow {
    let from = Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0).unwrap();
    let to = Utc.with_ymd_and_hms(2026, 5, 11, 0, 0, 0).unwrap();
    ExportWindow::new(from, to).expect("from <= to")
}

#[test]
fn happy_path_emits_chain_proof_and_renders_both_formats() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let stored = build_chain(&chain, synthetic_entries());
    let expected_end = hex::encode(&stored.last().unwrap().hmac);

    let bundle = export_bundle(
        Framework::Soc2Cc72,
        "t-acme".into(),
        stored.clone(),
        window(),
        &chain,
    )
    .expect("happy path bundle");

    // Chain proof reflects the full window slice.
    assert_eq!(bundle.header.chain_proof.first_id, 1);
    assert_eq!(bundle.header.chain_proof.last_id, 5);
    assert_eq!(bundle.header.chain_proof.count, 5);
    assert_eq!(
        bundle.header.chain_proof.start_prev_hmac_hex,
        "00".repeat(HMAC_LEN),
        "first slice row has the genesis prev_hmac"
    );
    assert_eq!(bundle.header.chain_proof.end_hmac_hex, expected_end);

    // JSON renders and round-trips through serde_json.
    let json = render_json(&bundle).expect("render json");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert_eq!(
        parsed["header"]["framework"].as_str(),
        Some("soc2-cc72"),
        "framework serializes as kebab-case"
    );
    assert!(parsed["header"]["chain_proof"]["end_hmac_hex"].is_string());

    // CSV renders, row count matches.
    let csv = render_csv(&bundle).expect("render csv");
    let data_rows = csv
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("id,"))
        .count();
    assert_eq!(data_rows, bundle.rows.len());
}

#[test]
fn tampering_one_row_refuses_export_with_correct_row_id() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let mut stored = build_chain(&chain, synthetic_entries());
    // Tamper row id=3 (memory.recall) — mutate details but leave the stored
    // HMAC bytes alone so verify_chain finds HmacMismatch at id 3.
    stored[2].entry.details = json!({"q":"injected","tampered":true});

    let err = export_bundle(
        Framework::GdprArt30,
        "t-acme".into(),
        stored,
        window(),
        &chain,
    )
    .expect_err("must refuse tampered chain");

    let serialized = serde_json::to_string(&err).expect("error JSON-serializable");
    assert!(serialized.contains("chain_broken"));

    match err {
        ExportError::ChainBroken {
            first_broken_id,
            first_broken_ts,
        } => {
            assert_eq!(first_broken_id, 3);
            // ts came from the (tampered) row at id 3 in the window.
            let expected = Utc.with_ymd_and_hms(2026, 5, 10, 9, 1, 0).unwrap();
            assert_eq!(first_broken_ts, expected);
        }
        other => panic!("expected ChainBroken, got {other:?}"),
    }
}

#[test]
fn all_three_frameworks_yield_a_renderable_bundle() {
    let chain = ChainedAudit::new(KEY.to_vec());
    let stored = build_chain(&chain, synthetic_entries());

    for fw in [
        Framework::Soc2Cc72,
        Framework::GdprArt30,
        Framework::Hipaa164312,
    ] {
        let bundle = export_bundle(fw, "t-acme".into(), stored.clone(), window(), &chain)
            .unwrap_or_else(|e| panic!("export {fw:?}: {e}"));

        // Chain proof is identical across frameworks — they project the same
        // underlying slice and so share the same window cryptographic boundary.
        assert_eq!(bundle.header.chain_proof.first_id, 1);
        assert_eq!(bundle.header.chain_proof.last_id, 5);

        // Both renderers succeed.
        let json = render_json(&bundle).expect("render json");
        let csv = render_csv(&bundle).expect("render csv");
        assert!(json.contains("chain_proof"));
        assert!(csv.starts_with("# bundle-header"));

        // Sprint-8 S8-6: Format::Pdf now returns real PDF bytes via the
        // pdf-writer backend. Reproducibility is asserted in
        // src/pdf.rs::tests; here we only confirm the convenience wrapper
        // surfaces non-empty bytes that look like a PDF.
        let pdf = xiaoguai_audit::render(&bundle, Format::Pdf).expect("pdf render");
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
