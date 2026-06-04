//! Semantic recall accuracy tests using `InMemoryEmbedder`.
//!
//! ## How deterministic similarity works
//!
//! `InMemoryEmbedder` maps text to an f32 vector via a polynomial hash that
//! accumulates each byte into all dimension buckets, then L2-normalises the
//! result. Two strings that share many bytes at the same positions will produce
//! vectors with high cosine similarity; strings that are byte-disjoint will
//! score low.
//!
//! The fixtures below exploit this property:
//! - "cats are great pets" and "cats make wonderful companions" share the
//!   prefix "cats" and similar lengths → high similarity.
//! - "the weather is cold today" shares nothing byte-wise with either → low
//!   similarity.
//!
//! We assert on **ordering** (top result is more relevant than bottom), not on
//! exact scores, which avoids brittleness from floating-point rounding.

use std::sync::Arc;

use uuid::Uuid;

use crate::embedder::InMemoryEmbedder;
use crate::store::InMemoryMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, MemoryKind, RecallRequest};

fn store() -> InMemoryMemoryStore {
    InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::new(64)))
}

#[tokio::test]
async fn recall_returns_most_similar_first() {
    let s = store();

    // Insert three memories with different content.
    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: "cats are great pets and companions".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: "cats make wonderful animal friends".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: "quantum mechanics describes subatomic particle behaviour".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let recalled = s
        .recall_memories(RecallRequest {
            query: "cats are great as pets".to_owned(),
            top_k: 3,
            kind_filter: None,
            tag_filter: vec![],
            session_id: None,
        })
        .await
        .unwrap();

    assert_eq!(recalled.len(), 3);

    // Top-1 must score higher than the quantum physics entry.
    let top_score = recalled[0].score;
    let bottom_score = recalled[recalled.len() - 1].score;
    assert!(
        top_score >= bottom_score,
        "results must be in descending score order: top={top_score}, bottom={bottom_score}"
    );

    // The quantum entry must not win over cat-related content.
    let quantum_pos = recalled
        .iter()
        .position(|r| r.memory.content.starts_with("quantum"))
        .expect("quantum entry must appear");
    assert!(
        quantum_pos > 0,
        "quantum physics should not be the top recall result for a cats query"
    );
}

#[tokio::test]
async fn recall_updates_metadata() {
    let s = store();

    let m = s
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Episodes,
            content: "session summary from 2026-01-01".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    assert_eq!(m.recall_count, 0);
    assert!(m.last_recalled_at.is_none());

    s.recall_memories(RecallRequest {
        query: "session summary".to_owned(),
        top_k: 5,
        kind_filter: None,
        tag_filter: vec![],
        session_id: Some(Uuid::new_v4()),
    })
    .await
    .unwrap();

    let updated = s.get_memory(m.id).await.unwrap();
    assert_eq!(updated.recall_count, 1);
    assert!(updated.last_recalled_at.is_some());
}

#[tokio::test]
async fn recall_respects_top_k() {
    let s = store();

    for i in 0..10u32 {
        s.create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: format!("memory number {i} about something interesting"),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();
    }

    let recalled = s
        .recall_memories(RecallRequest {
            query: "interesting memory".to_owned(),
            top_k: 3,
            kind_filter: None,
            tag_filter: vec![],
            session_id: None,
        })
        .await
        .unwrap();

    assert_eq!(recalled.len(), 3, "top_k=3 must return exactly 3 results");
}

#[tokio::test]
async fn find_similar_excludes_self() {
    let s = store();

    let anchor = s
        .create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: "user prefers dark mode interface".to_owned(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: "user prefers dark color scheme".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let similar = s.find_similar(anchor.id, 5).await.unwrap();

    assert!(
        similar.iter().all(|r| r.memory.id != anchor.id),
        "find_similar must not return the anchor memory itself"
    );
}

#[tokio::test]
async fn recall_with_kind_filter() {
    let s = store();

    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: "user is based in London".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    s.create_memory(CreateMemoryRequest {
        kind: MemoryKind::Episodes,
        content: "user is based in London, discussed budget".to_owned(),
        tags: vec![],
        ttl_at: None,
    })
    .await
    .unwrap();

    let recalled = s
        .recall_memories(RecallRequest {
            query: "user location London".to_owned(),
            top_k: 10,
            kind_filter: Some(MemoryKind::Facts),
            tag_filter: vec![],
            session_id: None,
        })
        .await
        .unwrap();

    assert!(
        recalled.iter().all(|r| r.memory.kind == MemoryKind::Facts),
        "kind_filter=facts must exclude episodes"
    );
}
