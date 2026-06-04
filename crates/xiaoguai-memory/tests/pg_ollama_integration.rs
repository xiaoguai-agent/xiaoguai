//! Integration tests for `SqliteMemoryStore` against a temp `SQLite` database
//! (DEC-033 single-user pivot). pgvector is gone: embeddings are stored as a
//! BLOB of 384 little-endian f32 and cosine similarity is scanned in Rust.
//!
//! Two tests live here:
//!
//!  * [`sqlite_memory_crud_and_cosine_recall`] — DB-only. Uses the deterministic
//!    [`InMemoryEmbedder`] (no network) so it runs on every `cargo test
//!    --features ollama`. It exercises the real `SQLite` store path (create /
//!    get / recall) and asserts cosine ranking returns the closest memory
//!    first.
//!
//!  * [`pg_ollama_semantic_recall_ranks_closest_first`] — kept `#[ignore]`
//!    because it needs a live Ollama server (`ollama pull all-minilm`). It
//!    compiles against the `SQLite` store; run it explicitly with
//!    `cargo test -p xiaoguai-memory --features ollama -- --ignored`.
//!
//! The `tenant_id` column / field was dropped under the single-user pivot.

#![cfg(all(feature = "pg", feature = "ollama"))]

use std::sync::Arc;

use sqlx::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;
use xiaoguai_memory::{
    types::{CreateMemoryRequest, RecallRequest},
    InMemoryEmbedder, MemoryKind, MemoryStore, OllamaEmbedder, SqliteMemoryStore,
};
use xiaoguai_storage::db;

const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";
const EXPECTED_DIM: usize = 384;

/// Returns a connected+migrated temp `SQLite` pool. The returned `TempDir` must
/// stay alive for the duration of the test (dropping it deletes the DB file).
async fn setup() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("t.db");
    let pool = db::connect(path.to_str().expect("utf8 path"), 4)
        .await
        .expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, dir)
}

fn create_req(content: &str, tags: &[&str]) -> CreateMemoryRequest {
    CreateMemoryRequest {
        kind: MemoryKind::Facts,
        content: content.to_owned(),
        tags: tags.iter().map(|s| (*s).to_owned()).collect(),
        ttl_at: None,
    }
}

/// DB-only path: real `SQLite` store + deterministic embedder. No network.
#[tokio::test]
async fn sqlite_memory_crud_and_cosine_recall() {
    let (pool, _dir) = setup().await;
    let embedder = Arc::new(InMemoryEmbedder::new(EXPECTED_DIM));
    let store = SqliteMemoryStore::new(pool, embedder);

    let cat = store
        .create_memory(create_req(
            "My cat Mittens is a fluffy orange tabby who loves to nap in the sun.",
            &["pet"],
        ))
        .await
        .expect("create cat memory");
    store
        .create_memory(create_req(
            "The production PostgreSQL database is hosted in the us-east-1 region.",
            &["infra"],
        ))
        .await
        .expect("create database memory");
    store
        .create_memory(create_req(
            "I prefer drinking my morning coffee black with no sugar.",
            &["habit"],
        ))
        .await
        .expect("create coffee memory");

    assert_eq!(
        cat.content_embedding.len(),
        EXPECTED_DIM,
        "InMemoryEmbedder should produce {EXPECTED_DIM}-dim vectors"
    );

    // The deterministic embedder is content-stable, so an exact-content query
    // is the nearest neighbour of itself: cosine recall must rank `cat` first.
    let recalled = store
        .recall_memories(RecallRequest {
            query: "My cat Mittens is a fluffy orange tabby who loves to nap in the sun."
                .to_owned(),
            top_k: 3,
            kind_filter: None,
            tag_filter: Vec::new(),
            session_id: Some(Uuid::new_v4()),
        })
        .await
        .expect("recall memories");

    assert!(
        !recalled.is_empty(),
        "recall returned no memories — cosine scan did not run"
    );
    assert_eq!(
        recalled[0].memory.id, cat.id,
        "closest memory should be the cat fact; got content: {:?}",
        recalled[0].memory.content
    );
    for w in recalled.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "scores must be sorted descending: {} < {}",
            w[0].score,
            w[1].score
        );
    }
    assert!(
        (0.0..=1.0).contains(&recalled[0].score),
        "score should be in [0,1], got {}",
        recalled[0].score
    );

    // Reading the memory back surfaces the persisted 384-dim embedding.
    let fetched = store.get_memory(cat.id).await.expect("get_memory");
    assert_eq!(
        fetched.content_embedding.len(),
        EXPECTED_DIM,
        "persisted embedding should round-trip as {EXPECTED_DIM}-dim, got {}",
        fetched.content_embedding.len()
    );
}

/// Live-network path: real `OllamaEmbedder` (`all-minilm`, 384-dim) against the
/// `SQLite` store. `#[ignore]`'d — needs a running Ollama server.
#[tokio::test]
#[ignore = "requires a running Ollama with all-minilm; set OLLAMA_HOST"]
async fn pg_ollama_semantic_recall_ranks_closest_first() {
    let ollama_host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_OLLAMA_HOST.to_owned());

    let (pool, _dir) = setup().await;
    let embedder = Arc::new(OllamaEmbedder::from_host(&ollama_host));
    let store = SqliteMemoryStore::new(pool, embedder);

    let cat = store
        .create_memory(create_req(
            "My cat Mittens is a fluffy orange tabby who loves to nap in the sun.",
            &["pet"],
        ))
        .await
        .expect("create cat memory");
    store
        .create_memory(create_req(
            "The production PostgreSQL database is hosted in the us-east-1 region.",
            &["infra"],
        ))
        .await
        .expect("create database memory");
    store
        .create_memory(create_req(
            "I prefer drinking my morning coffee black with no sugar.",
            &["habit"],
        ))
        .await
        .expect("create coffee memory");

    assert_eq!(
        cat.content_embedding.len(),
        EXPECTED_DIM,
        "OllamaEmbedder(all-minilm) should produce {EXPECTED_DIM}-dim vectors, got {}",
        cat.content_embedding.len()
    );

    // A query semantically close to the "cat" memory.
    let recalled = store
        .recall_memories(RecallRequest {
            query: "Tell me about my kitten.".to_owned(),
            top_k: 3,
            kind_filter: None,
            tag_filter: Vec::new(),
            session_id: Some(Uuid::new_v4()),
        })
        .await
        .expect("recall memories");

    assert!(
        !recalled.is_empty(),
        "recall returned no memories — cosine scan did not run"
    );
    assert_eq!(
        recalled[0].memory.id, cat.id,
        "closest memory should be the cat fact; got content: {:?}",
        recalled[0].memory.content
    );
    for w in recalled.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "scores must be sorted descending: {} < {}",
            w[0].score,
            w[1].score
        );
    }
    assert!(
        (0.0..=1.0).contains(&recalled[0].score),
        "score should be in [0,1], got {}",
        recalled[0].score
    );

    let fetched = store.get_memory(cat.id).await.expect("get_memory");
    assert_eq!(
        fetched.content_embedding.len(),
        EXPECTED_DIM,
        "persisted embedding should round-trip as {EXPECTED_DIM}-dim, got {}",
        fetched.content_embedding.len()
    );
}
