//! Integration tests for the Tier-1b in-process cache fallback.
//!
//! Unlike `tests/cache.rs` (containerized Redis, gated behind `#[ignore]`),
//! these run unconditionally — the in-process backend has no external
//! dependencies, which is the entire point of Tier-1b.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use xiaoguai_storage::cache::Cache;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Item {
    id: u32,
    label: String,
}

#[tokio::test]
async fn boots_in_process_cache_end_to_end() {
    // This is the exact call shape that `xiaoguai_core::run_smoke` performs.
    let cache = Cache::new("xiaoguai:");

    // Mirror the smoke heartbeat write — must succeed without any broker.
    let ts = "2026-05-28T12:00:00Z".to_string();
    cache
        .set("smoke/heartbeat", &ts, Some(Duration::from_secs(60)))
        .await
        .expect("set heartbeat");
    let got: Option<String> = cache.get("smoke/heartbeat").await.expect("get heartbeat");
    assert_eq!(got.as_deref(), Some(ts.as_str()));
}

#[tokio::test]
async fn in_process_cache_supports_full_typed_workflow() {
    let cache = Cache::new("xiaoguai:");

    let v = Item {
        id: 1,
        label: "first".into(),
    };
    cache.set("items/1", &v, None).await.expect("set");

    // Counter path.
    assert_eq!(cache.incr("hits", 1).await.expect("incr"), 1);
    assert_eq!(cache.incr("hits", 4).await.expect("incr"), 5);

    // Delete behaviour.
    assert!(cache.delete("items/1").await.expect("delete existing"));
    assert!(!cache.delete("items/1").await.expect("delete missing"));
}
