//! Property-based tests for the HMAC chain. No Postgres required.

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use serde_json::json;
use xiaoguai_audit::chain::{AuditEntry, ChainedAudit, StoredEntry, HMAC_LEN};

const KEY: &[u8] = b"proptest-key";

fn arb_simple_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        any::<i32>().prop_map(|n| json!(n)),
        any::<bool>().prop_map(|b| json!(b)),
        "[a-z]{0,8}".prop_map(|s| json!(s)),
        Just(serde_json::Value::Null),
    ]
}

fn arb_details() -> impl Strategy<Value = serde_json::Value> {
    prop::collection::hash_map("[a-z]{1,6}", arb_simple_value(), 0..6)
        .prop_map(|m| json!(m.into_iter().collect::<serde_json::Map<_, _>>()))
}

prop_compose! {
    fn arb_entry()(
        tenant in "[a-z]{1,8}",
        actor in "[a-z]{1,8}",
        action in "[a-z]{1,12}",
        resource_present in any::<bool>(),
        resource in "[a-z0-9]{0,12}",
        ts_secs in 1_700_000_000i64..1_900_000_000i64,
        details in arb_details(),
    ) -> AuditEntry {
        AuditEntry {
            ts: Utc.timestamp_opt(ts_secs, 0).single().unwrap(),
            tenant_id: tenant,
            actor,
            action,
            resource: if resource_present { Some(resource) } else { None },
            details,
        }
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    /// Property 1: Any computed chain verifies successfully.
    #[test]
    fn prop_computed_chain_verifies(entries in prop::collection::vec(arb_entry(), 0..16)) {
        let chain = ChainedAudit::new(KEY.to_vec());
        let stored = build_chain(&chain, entries);
        let zero = [0u8; HMAC_LEN];
        chain.verify_chain(&zero, &stored).expect("computed chain must verify");
    }

    /// Property 2: Tampering with any single entry's details breaks verification.
    #[test]
    fn prop_tampered_details_break_chain(
        entries in prop::collection::vec(arb_entry(), 1..10),
        target_idx in 0usize..10,
    ) {
        let chain = ChainedAudit::new(KEY.to_vec());
        let mut stored = build_chain(&chain, entries);
        let idx = target_idx % stored.len();
        // Replace details with a sentinel guaranteed to differ from the original
        // (we wrap in a unique object key so equality is essentially impossible).
        stored[idx].entry.details = json!({ "__proptest_tampered__": idx as u64 });
        let zero = [0u8; HMAC_LEN];
        let result = chain.verify_chain(&zero, &stored);
        prop_assert!(result.is_err(), "tampering at index {idx} should break the chain");
    }

    /// Property 3: Reordering breaks the chain when entries are non-trivially distinct.
    #[test]
    fn prop_reordering_breaks_chain(
        entries in prop::collection::vec(arb_entry(), 2..8),
    ) {
        let chain = ChainedAudit::new(KEY.to_vec());
        let stored = build_chain(&chain, entries);
        // Swap first two entries — their prev_hmac fields no longer line up
        // with the (zero, hmac[0]) sequence.
        let mut reordered = stored.clone();
        reordered.swap(0, 1);
        let zero = [0u8; HMAC_LEN];
        let result = chain.verify_chain(&zero, &reordered);
        prop_assert!(result.is_err(), "reordering must break the chain");
    }

    /// Property 4: HMAC is deterministic across recomputation.
    #[test]
    fn prop_hmac_deterministic(entry in arb_entry(), prev_seed in any::<u8>()) {
        let chain = ChainedAudit::new(KEY.to_vec());
        let prev = vec![prev_seed; HMAC_LEN];
        let a = chain.compute_hmac(&prev, &entry).unwrap();
        let b = chain.compute_hmac(&prev, &entry).unwrap();
        prop_assert_eq!(a, b);
    }
}
