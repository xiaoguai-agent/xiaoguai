-- MCP server manifest registry (SQLite single-user). tenant_id + RLS dropped.
-- Secrets policy: only env-var NAMES are stored (`env_keys`), never values.

CREATE TABLE mcp_servers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL DEFAULT '0.0.0',
    transport   TEXT NOT NULL,                 -- 'stdio' | 'sse' | 'http'
    command     TEXT,                          -- stdio only
    args        TEXT NOT NULL DEFAULT '[]',
    env_keys    TEXT NOT NULL DEFAULT '[]',
    endpoint    TEXT,                          -- sse / http only
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX ux_mcp_servers_name_version ON mcp_servers (name, version);
CREATE INDEX ix_mcp_servers_enabled ON mcp_servers (enabled);
