//! TTL expiry tests.

use std::sync::Arc;

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::embedder::InMemoryEmbedder;
use crate::store::InMemoryMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, MemoryKind};

fn store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::new(16)))
}

fn tenant() -> Uuid {
    Uuid::new_v4()
}

#[tokio::test]
async fn cleanup_removes_expired_only() {
    let s = store();
    let tid = tenant();

    // Already-expired TTL (1 hour in the past).
    let expired = s
        .create_memory(CreateMemoryRequest {
            tenant_id: tid,
            kind: MemoryKind::Facts,
            content: "this memory is expired".to_owned(),
            tags: vec![],
            ttl_at: Some(Utc::now() - Duration::hours(1)),
        })
        .await
        .unwrap();

    // Future TTL (survives cleanup).
    let alive = s
        .create_memory(CreateMemoryRequest {
            tenant_id: tid,
            kind: MemoryKind::Facts,
            content: "this memory is still alive".to_owned(),
            tags: vec![],
            ttl_at: Some(Utc::now() + Duration::hours(24)),
        })
        .await
        .unwrap();

    // No-TTL memory (never expires).
    let eternal = s
        .create_memory(CreateMemoryRequest {
            tenant_id: tid,
            kind: MemoryKind::Facts,
            content: "this memory never expires".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let removed = s.cleanup_expired().await.unwrap();
    assert_eq!(removed, 1, "only the expired memory should be removed");

    assert!(
        matches!(
            s.get_memory(expired.id, tid).await,
            Err(crate::MemoryError::NotFound(_))
        ),
        "expired memory should be gone"
    );
    assert!(
        s.get_memory(alive.id, tid).await.is_ok(),
        "alive memory should still exist"
    );
    assert!(
        s.get_memory(eternal.id, tid).await.is_ok(),
        "eternal memory should still exist"
    );
}

#[tokio::test]
async fn cleanup_is_idempotent() {
    let s = store();
    let tid = tenant();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Episodes,
        content: "stale episode".to_owned(),
        tags: vec![],
        ttl_at: Some(Utc::now() - Duration::minutes(5)),
    })
    .await
    .unwrap();

    let removed1 = s.cleanup_expired().await.unwrap();
    let removed2 = s.cleanup_expired().await.unwrap();

    assert_eq!(removed1, 1);
    assert_eq!(removed2, 0, "second cleanup must find nothing to remove");
}

#[tokio::test]
async fn no_ttl_survives_cleanup() {
    let s = store();
    let tid = tenant();

    let m = s
        .create_memory(CreateMemoryRequest {
            tenant_id: tid,
            kind: MemoryKind::Preferences,
            content: "prefers no expiry".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let removed = s.cleanup_expired().await.unwrap();
    assert_eq!(removed, 0);
    assert!(s.get_memory(m.id, tid).await.is_ok());
}
