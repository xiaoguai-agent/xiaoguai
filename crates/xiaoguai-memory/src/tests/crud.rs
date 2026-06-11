//! CRUD tests for `InMemoryMemoryStore`.

use std::sync::Arc;

use uuid::Uuid;

use crate::embedder::InMemoryEmbedder;
use crate::store::InMemoryMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, MemoryKind, UpdateMemoryRequest};

fn make_store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::new(16)))
}

#[tokio::test]
async fn create_and_get_round_trip() {
    let store = make_store();

    let created = store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: "User's name is Alice".to_owned(),
            tags: vec!["user".to_owned(), "name".to_owned()],
            ttl_at: None,
        })
        .await
        .expect("create");

    assert_eq!(created.kind, MemoryKind::Facts);
    assert_eq!(created.recall_count, 0);
    assert!(
        !created.content_embedding.is_empty(),
        "embedding must be populated"
    );

    let fetched = store.get_memory(created.id).await.expect("get");

    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.content, "User's name is Alice");
}

#[tokio::test]
async fn list_with_kind_filter() {
    let store = make_store();

    store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: "fact one".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Episodes,
            content: "episode one".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let facts = store
        .list_memories(Some(MemoryKind::Facts), &[], 50, 0)
        .await
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].kind, MemoryKind::Facts);

    let all = store.list_memories(None, &[], 50, 0).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn update_content_re_embeds() {
    let store = make_store();

    let created = store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Preferences,
            content: "prefers dark mode".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let old_emb = created.content_embedding.clone();

    let updated = store
        .update_memory(
            created.id,
            UpdateMemoryRequest {
                content: Some("prefers light mode".to_owned()),
                tags: None,
                ttl_at: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.content, "prefers light mode");
    // Embedding must change when content changes.
    assert_ne!(
        updated.content_embedding, old_emb,
        "embedding should be re-computed on update"
    );
}

#[tokio::test]
async fn delete_removes_memory() {
    let store = make_store();

    let created = store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Episodes,
            content: "episode to delete".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    store.delete_memory(created.id).await.expect("delete");

    let err = store.get_memory(created.id).await;
    assert!(
        matches!(err, Err(crate::MemoryError::NotFound(_))),
        "expected NotFound after delete"
    );
}

#[tokio::test]
async fn get_nonexistent_returns_not_found() {
    let store = make_store();
    let id = Uuid::new_v4();
    let err = store.get_memory(id).await;
    assert!(matches!(err, Err(crate::MemoryError::NotFound(x)) if x == id));
}

#[tokio::test]
async fn delete_nonexistent_returns_not_found() {
    let store = make_store();
    let id = Uuid::new_v4();
    let err = store.delete_memory(id).await;
    assert!(matches!(err, Err(crate::MemoryError::NotFound(_))));
}

#[tokio::test]
async fn list_pagination() {
    let store = make_store();

    for i in 0..5u32 {
        store
            .create_memory(CreateMemoryRequest {
                kind: MemoryKind::Facts,
                content: format!("fact {i}"),
                tags: vec![],
                ttl_at: None,
            })
            .await
            .unwrap();
    }

    let page1 = store.list_memories(None, &[], 3, 0).await.unwrap();
    let page2 = store.list_memories(None, &[], 3, 3).await.unwrap();

    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 2);
    // No overlap.
    let ids1: std::collections::HashSet<_> = page1.iter().map(|m| m.id).collect();
    let ids2: std::collections::HashSet<_> = page2.iter().map(|m| m.id).collect();
    assert!(ids1.is_disjoint(&ids2));
}

// ─── #288: content byte cap at the write boundary ───────────────────────────

#[tokio::test]
async fn create_rejects_content_over_the_byte_cap() {
    let store = make_store();
    let big = "x".repeat(crate::types::MAX_CONTENT_BYTES + 1);
    let err = store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: big,
            tags: vec![],
            ttl_at: None,
        })
        .await;
    assert!(matches!(err, Err(crate::MemoryError::InvalidArgument(_))));
    // Nothing landed.
    assert!(store
        .list_memories(None, &[], 10, 0)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn create_accepts_content_at_exactly_the_byte_cap() {
    let store = make_store();
    let max = "x".repeat(crate::types::MAX_CONTENT_BYTES);
    store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: max,
            tags: vec![],
            ttl_at: None,
        })
        .await
        .expect("content at the cap is allowed");
}

#[tokio::test]
async fn update_rejects_content_over_the_byte_cap() {
    let store = make_store();
    let created = store
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: "small".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let err = store
        .update_memory(
            created.id,
            UpdateMemoryRequest {
                content: Some("x".repeat(crate::types::MAX_CONTENT_BYTES + 1)),
                tags: None,
                ttl_at: None,
            },
        )
        .await;
    assert!(matches!(err, Err(crate::MemoryError::InvalidArgument(_))));
    // Original content untouched.
    let fetched = store.get_memory(created.id).await.unwrap();
    assert_eq!(fetched.content, "small");
}
