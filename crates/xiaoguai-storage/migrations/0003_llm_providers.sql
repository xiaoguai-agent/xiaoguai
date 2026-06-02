-- LLM provider registry (SQLite single-user). One owner; every row is visible.
-- The Postgres tenant-vs-global split (tenant_id NULL = global) collapses to a
-- single namespace, so names are simply unique across the table.

CREATE TABLE llm_providers (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    kind                TEXT NOT NULL,
    endpoint            TEXT NOT NULL,
    models              TEXT NOT NULL DEFAULT '[]',
    default_for_models  TEXT NOT NULL DEFAULT '[]',
    fallback_order      INTEGER NOT NULL DEFAULT 100,
    api_key_env         TEXT,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX ux_llm_providers_name ON llm_providers (name);
CREATE INDEX ix_llm_providers_fallback ON llm_providers (fallback_order);
