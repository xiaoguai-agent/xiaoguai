//! Tag filter tests for `list_memories` and `recall_memories`.

use std::sync::Arc;

use uuid::Uuid;

use crate::embedder::InMemoryEmbedder;
use crate::store::InMemoryMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, MemoryKind, RecallRequest};

fn store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::new(16)))
}

fn tenant() -> Uuid {
    Uuid::new_v4()
}

#[tokio::test]
async fn list_with_tag_filter() {
    let s = store();
    let tid = tenant();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "tagged memory with project tag".to_owned(),
        tags: vec!["project".to_owned(), "alpha".to_owned()],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "untagged memory".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let tagged = s
        .list_memories(tid, None, &["project".to_owned()], 50, 0)
        .await
        .unwrap();

    assert_eq!(tagged.len(), 1, "only the tagged memory should appear");
    assert!(tagged[0].tags.contains(&"project".to_owned()));
}

#[tokio::test]
async fn list_requires_all_tags() {
    let s = store();
    let tid = tenant();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "has both tags".to_owned(),
        tags: vec!["a".to_owned(), "b".to_owned()],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "has only tag a".to_owned(),
        tags: vec!["a".to_owned()],
        ttl_at: None,
    })
    .await
    .unwrap();

    // Requesting both tags should only return the first memory.
    let both = s
        .list_memories(tid, None, &["a".to_owned(), "b".to_owned()], 50, 0)
        .await
        .unwrap();
    assert_eq!(both.len(), 1);
    assert_eq!(both[0].content, "has both tags");
}

#[tokio::test]
async fn recall_with_tag_filter() {
    let s = store();
    let tid = tenant();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "user is a premium subscriber".to_owned(),
        tags: vec!["billing".to_owned()],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tid,
        kind: MemoryKind::Facts,
        content: "user is a premium subscriber and has premium features".to_owned(),
        tags: vec!["billing".to_owned(), "features".to_owned()],
        ttl_at: None,
    })
    .await
    .unwrap();

    let recalled = s
        .recall_memories(RecallRequest {
            tenant_id: tid,
            query: "premium user subscription".to_owned(),
            top_k: 10,
            kind_filter: None,
            tag_filter: vec!["features".to_owned()],
            session_id: None,
        })
        .await
        .unwrap();

    assert_eq!(
        recalled.len(),
        1,
        "only the memory with 'features' tag should appear"
    );
    assert!(recalled[0].memory.tags.contains(&"features".to_owned()));
}
