-- v1.3.x: long-term agent memory with pgvector semantic retrieval.
--
-- Prerequisites:
--   CREATE EXTENSION IF NOT EXISTS vector;   -- pgvector must be installed
--
-- Tables:
--   memories      — per-tenant memory records with HNSW vector index
--   recall_traces — observability log of every recall invocation
--
-- Embedding dimension: 384 (matches sentence-transformers/all-MiniLM-L6-v2
-- and the InMemoryEmbedder test fixture). Change to 1536 for text-embedding-ada-002.

-- pgvector must be available in the target Postgres. This was previously only
-- a comment in the header, so the `vector` type below failed on a clean DB
-- (and the migrations-smoke test). IF NOT EXISTS keeps it idempotent.
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE memories (
    id                  UUID            PRIMARY KEY,
    tenant_id           UUID            NOT NULL,
    -- enum: facts | episodes | preferences (enforced at application layer)
    kind                TEXT            NOT NULL,
    content             TEXT            NOT NULL,
    -- pgvector column; HNSW index below
    content_embedding   vector(384)     NOT NULL,
    tags                TEXT[]          NOT NULL DEFAULT '{}',
    -- NULL = never expires
    ttl_at              TIMESTAMPTZ,
    created_at          TIMESTAMPTZ     NOT NULL DEFAULT now(),
    last_recalled_at    TIMESTAMPTZ,
    recall_count        INT             NOT NULL DEFAULT 0
);

-- Semantic similarity: HNSW with cosine distance (pgvector ≥ 0.5).
-- ef_construction=128 / m=16 are safe production defaults; tune for your
-- dataset once you have recall latency SLOs.
CREATE INDEX memories_embedding_hnsw_idx
    ON memories USING hnsw (content_embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 128);

-- Tenant + kind filter (covers list_memories and recall kind_filter).
CREATE INDEX memories_tenant_kind_idx
    ON memories (tenant_id, kind);

-- Tag containment filter via GIN (supports @> operator in recall_memories).
CREATE INDEX memories_tags_gin_idx
    ON memories USING gin (tags);

-- TTL sweep: partial index limits the cleanup scan to expiring rows.
CREATE INDEX memories_ttl_idx
    ON memories (ttl_at)
    WHERE ttl_at IS NOT NULL;

-- ─── Recall traces ──────────────────────────────────────────────────────────

CREATE TABLE recall_traces (
    id                  UUID            PRIMARY KEY,
    -- NULL when invoked outside a session (e.g. background sweep).
    session_id          UUID,
    query_embedding     vector(384)     NOT NULL,
    -- Array of {id, score} objects for the memories returned.
    memories_recalled   JSONB           NOT NULL DEFAULT '[]'::jsonb,
    recalled_at         TIMESTAMPTZ     NOT NULL DEFAULT now()
);

-- Support time-range queries on the traces table.
CREATE INDEX recall_traces_recalled_at_idx
    ON recall_traces (recalled_at DESC);

-- Support per-session recall history lookup.
CREATE INDEX recall_traces_session_idx
    ON recall_traces (session_id)
    WHERE session_id IS NOT NULL;
