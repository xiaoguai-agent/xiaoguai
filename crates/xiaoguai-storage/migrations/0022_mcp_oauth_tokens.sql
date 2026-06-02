-- Tier-3 T4: OAuth 2.1 PKCE token storage for outbound MCP servers (SQLite single-user).
-- Adds `auth` (TEXT/JSON) to mcp_servers + a token table. tenant_id + RLS dropped;
-- UNIQUE(server_id,tenant_id) collapses to UNIQUE(server_id).

ALTER TABLE mcp_servers ADD COLUMN auth TEXT;

CREATE TABLE mcp_oauth_tokens (
    id            TEXT PRIMARY KEY,
    server_id     TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    access_token  TEXT NOT NULL,
    refresh_token TEXT,
    expires_at    TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id)
);

CREATE INDEX ix_mcp_oauth_tokens_server ON mcp_oauth_tokens (server_id);
