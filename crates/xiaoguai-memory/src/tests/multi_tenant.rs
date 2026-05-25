//! Multi-tenant isolation tests.
//!
//! Ensures that Tenant A cannot read, recall, or delete Tenant B's memories.

use std::sync::Arc;

use uuid::Uuid;

use crate::embedder::InMemoryEmbedder;
use crate::store::InMemoryMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, MemoryKind, RecallRequest};

fn store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::new(32)))
}

#[tokio::test]
async fn list_does_not_cross_tenant_boundary() {
    let s = store();
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tenant_a,
        kind: MemoryKind::Facts,
        content: "tenant A secret".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tenant_b,
        kind: MemoryKind::Facts,
        content: "tenant B secret".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let a_memories = s.list_memories(tenant_a, None, &[], 50, 0).await.unwrap();
    let b_memories = s.list_memories(tenant_b, None, &[], 50, 0).await.unwrap();

    assert_eq!(a_memories.len(), 1);
    assert_eq!(b_memories.len(), 1);
    assert!(a_memories.iter().all(|m| m.tenant_id == tenant_a));
    assert!(b_memories.iter().all(|m| m.tenant_id == tenant_b));
}

#[tokio::test]
async fn get_with_wrong_tenant_returns_not_found() {
    let s = store();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();

    let m = s
        .create_memory(CreateMemoryRequest {
            tenant_id: owner,
            kind: MemoryKind::Facts,
            content: "owner-only data".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let err = s.get_memory(m.id, other).await;
    assert!(
        matches!(err, Err(crate::MemoryError::NotFound(_))),
        "wrong tenant should not see the memory"
    );
}

#[tokio::test]
async fn delete_with_wrong_tenant_returns_not_found() {
    let s = store();
    let owner = Uuid::new_v4();
    let attacker = Uuid::new_v4();

    let m = s
        .create_memory(CreateMemoryRequest {
            tenant_id: owner,
            kind: MemoryKind::Episodes,
            content: "owner episode".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    let err = s.delete_memory(m.id, attacker).await;
    assert!(
        matches!(err, Err(crate::MemoryError::NotFound(_))),
        "attacker should not be able to delete owner's memory"
    );

    // Memory must still be accessible by the owner.
    assert!(s.get_memory(m.id, owner).await.is_ok());
}

#[tokio::test]
async fn recall_does_not_cross_tenant_boundary() {
    let s = store();
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tenant_a,
        kind: MemoryKind::Facts,
        content: "cats are great pets".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        tenant_id: tenant_b,
        kind: MemoryKind::Facts,
        content: "cats are wonderful animals".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let recalled = s
        .recall_memories(RecallRequest {
            tenant_id: tenant_a,
            query: "cats".to_owned(),
            top_k: 10,
            kind_filter: None,
            tag_filter: vec![],
            session_id: None,
        })
        .await
        .unwrap();

    assert!(
        recalled.iter().all(|r| r.memory.tenant_id == tenant_a),
        "recall must only return memories belonging to the requesting tenant"
    );
}
