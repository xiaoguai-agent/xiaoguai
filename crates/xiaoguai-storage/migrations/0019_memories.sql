-- v1.3.x: long-term agent memory (SQLite single-user).
--
-- pgvector is gone: `content_embedding` / `query_embedding` are BLOB holding
-- 384 little-endian f32 values; cosine similarity is scanned in Rust (Phase 3).
-- The HNSW + GIN indexes are dropped. UUID -> TEXT; tenant_id dropped; text[] tags
-- -> TEXT holding a JSON array.

CREATE TABLE memories (
    id                  TEXT            PRIMARY KEY,
    -- enum: facts | episodes | preferences (enforced at application layer)
    kind                TEXT            NOT NULL,
    content             TEXT            NOT NULL,
    -- BLOB of 384 LE f32; cosine scanned in Rust.
    content_embedding   BLOB            NOT NULL,
    tags                TEXT            NOT NULL DEFAULT '[]',
    -- NULL = never expires
    ttl_at              TEXT,
    created_at          TEXT            NOT NULL DEFAULT (datetime('now')),
    last_recalled_at    TEXT,
    recall_count        INTEGER         NOT NULL DEFAULT 0
);

CREATE INDEX memories_kind_idx ON memories (kind);
CREATE INDEX memories_ttl_idx ON memories (ttl_at) WHERE ttl_at IS NOT NULL;

CREATE TABLE recall_traces (
    id                  TEXT            PRIMARY KEY,
    -- NULL when invoked outside a session (e.g. background sweep).
    session_id          TEXT,
    query_embedding     BLOB            NOT NULL,
    -- Array of {id, score} objects for the memories returned (JSON).
    memories_recalled   TEXT            NOT NULL DEFAULT '[]',
    recalled_at         TEXT            NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX recall_traces_recalled_at_idx ON recall_traces (recalled_at DESC);
CREATE INDEX recall_traces_session_idx ON recall_traces (session_id) WHERE session_id IS NOT NULL;
