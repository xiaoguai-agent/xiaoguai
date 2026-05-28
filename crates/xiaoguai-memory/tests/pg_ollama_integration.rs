//! Integration test: real `PgMemoryStore` against a real Postgres + pgvector
//! using the real `OllamaEmbedder` (`all-minilm`, 384-dim).
//!
//! This fills a coverage gap: the unit tests only exercise
//! `InMemoryMemoryStore` with the deterministic `InMemoryEmbedder`, so the
//! production pgvector path (HNSW cosine ranking) + a real network embedder are
//! never tested in CI. This test wires the two real components together and
//! asserts that semantic ranking actually works end-to-end.
//!
//! ## Requirements (why it is `#[ignore]`'d)
//!
//!  * A Postgres instance with the `vector` extension installed.
//!  * A running Ollama server with the `all-minilm` model pulled
//!    (`ollama pull all-minilm`).
//!
//! ## Schema
//!
//! The `memories` / `recall_traces` tables live in
//! `crates/xiaoguai-storage/migrations/0019_memories.sql`. We do **not** call
//! `xiaoguai_storage::db::migrate` here on purpose:
//!   * `db::migrate` runs `sqlx::migrate!("./migrations")` which would apply the
//!     entire storage migration chain (0001..0019), pulling in unrelated schema
//!     and requiring `xiaoguai-storage` as a dev-dependency.
//!   * Instead the test applies just the two tables it needs via raw sqlx,
//!     idempotently (`CREATE TABLE IF NOT EXISTS`). This keeps the test
//!     self-contained with no extra dependency and no dependency-cycle concern
//!     (storage does NOT depend on memory, but keeping memory free of a
//!     storage dev-dep is simpler regardless).
//!
//! ## Isolation & cleanup
//!
//! Every run uses a fresh random `tenant_id`, so concurrent runs and existing
//! data never interfere. On success (and on the explicit error paths) the test
//! deletes its own rows. A panic from a failed assertion leaves at most three
//! orphan rows under this run's unique random `tenant_id`, which can never
//! collide with another run or with real data.

#![cfg(all(feature = "pg", feature = "ollama"))]

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;
use xiaoguai_memory::{
    types::{CreateMemoryRequest, RecallRequest},
    MemoryKind, MemoryStore, OllamaEmbedder, PgMemoryStore,
};

const DEFAULT_DATABASE_URL: &str = "postgres://zw@localhost:5432/xiaoguai";
const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";
const EXPECTED_DIM: usize = 384;

/// Ensure the `memories` + `recall_traces` tables (and pgvector extension)
/// exist. Mirrors `0019_memories.sql` but uses `IF NOT EXISTS` so it is safe to
/// run against a DB that already has them.
async fn ensure_schema(pool: &PgPool) {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await
        .expect("failed to create the `vector` extension — is pgvector installed?");

    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS memories (
            id                  UUID         PRIMARY KEY,
            tenant_id           UUID         NOT NULL,
            kind                TEXT         NOT NULL,
            content             TEXT         NOT NULL,
            content_embedding   vector(384)  NOT NULL,
            tags                TEXT[]       NOT NULL DEFAULT '{}',
            ttl_at              TIMESTAMPTZ,
            created_at          TIMESTAMPTZ  NOT NULL DEFAULT now(),
            last_recalled_at    TIMESTAMPTZ,
            recall_count        INT          NOT NULL DEFAULT 0
        )
        ",
    )
    .execute(pool)
    .await
    .expect("failed to create `memories` table");

    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS recall_traces (
            id                  UUID         PRIMARY KEY,
            session_id          UUID,
            query_embedding     vector(384)  NOT NULL,
            memories_recalled   JSONB        NOT NULL DEFAULT '[]'::jsonb,
            recalled_at         TIMESTAMPTZ  NOT NULL DEFAULT now()
        )
        ",
    )
    .execute(pool)
    .await
    .expect("failed to create `recall_traces` table");
}

/// Delete this run's rows. Best-effort: logs but does not panic on failure so
/// it can run inside teardown after another assertion already failed.
async fn cleanup(pool: &PgPool, tenant_id: Uuid) {
    if let Err(e) = sqlx::query("DELETE FROM memories WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(pool)
        .await
    {
        eprintln!("cleanup: failed to delete memories for {tenant_id}: {e}");
    }
}

fn create_req(tenant_id: Uuid, content: &str, tags: &[&str]) -> CreateMemoryRequest {
    CreateMemoryRequest {
        tenant_id,
        kind: MemoryKind::Facts,
        content: content.to_owned(),
        tags: tags.iter().map(|s| (*s).to_owned()).collect(),
        ttl_at: None,
    }
}

#[tokio::test]
#[ignore = "requires Postgres+pgvector and a running Ollama with all-minilm; set DATABASE_URL + OLLAMA_HOST"]
async fn pg_ollama_semantic_recall_ranks_closest_first() {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let ollama_host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_OLLAMA_HOST.to_owned());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .unwrap_or_else(|e| panic!("connect to Postgres at {database_url} failed: {e}"));

    ensure_schema(&pool).await;

    let tenant_id = Uuid::new_v4();
    let embedder = Arc::new(OllamaEmbedder::from_host(&ollama_host));
    let store = PgMemoryStore::new(pool.clone(), embedder);

    // Run the body in an inner async fn that returns a Result. We propagate
    // setup errors as `Err` (so cleanup still runs) and use plain `assert!`
    // for the semantic-ranking checks. A panic from a failed assertion still
    // leaves at most three orphan rows under this run's unique random
    // `tenant_id`, which never collides with other runs/data; the explicit
    // cleanup below covers the success path and the early-return error paths.
    let body = async {
        // ── Store three semantically distinct memories ───────────────────────
        let cat = store
            .create_memory(create_req(
                tenant_id,
                "My cat Mittens is a fluffy orange tabby who loves to nap in the sun.",
                &["pet"],
            ))
            .await
            .expect("create cat memory");

        let _db = store
            .create_memory(create_req(
                tenant_id,
                "The production PostgreSQL database is hosted in the us-east-1 region.",
                &["infra"],
            ))
            .await
            .expect("create database memory");

        let _coffee = store
            .create_memory(create_req(
                tenant_id,
                "I prefer drinking my morning coffee black with no sugar.",
                &["habit"],
            ))
            .await
            .expect("create coffee memory");

        // The real embedder must produce 384-dim vectors (matches vector(384)).
        assert_eq!(
            cat.content_embedding.len(),
            EXPECTED_DIM,
            "OllamaEmbedder(all-minilm) should produce {EXPECTED_DIM}-dim vectors, got {}",
            cat.content_embedding.len()
        );

        // ── Recall with a query semantically close to the "cat" memory ───────
        let recalled = store
            .recall_memories(RecallRequest {
                tenant_id,
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
            "recall returned no memories — pgvector ranking did not run"
        );

        // Semantic ranking: the cat memory must come back first for a cat query.
        assert_eq!(
            recalled[0].memory.id, cat.id,
            "closest memory should be the cat fact; got content: {:?}",
            recalled[0].memory.content
        );

        // Scores are cosine similarity in [0, 1] and must be sorted descending.
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

        // Reading the memory back must surface the persisted 384-dim embedding.
        let fetched = store
            .get_memory(cat.id, tenant_id)
            .await
            .expect("get_memory");
        assert_eq!(
            fetched.content_embedding.len(),
            EXPECTED_DIM,
            "persisted embedding should round-trip as {EXPECTED_DIM}-dim, got {}",
            fetched.content_embedding.len()
        );
    };

    body.await;

    // Teardown: delete this run's rows and close the pool.
    cleanup(&pool, tenant_id).await;
    pool.close().await;
}
