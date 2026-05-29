-- Tier-3 T4: OAuth 2.1 PKCE token storage for outbound MCP servers.
--
-- Extends `mcp_servers` with an optional `auth` JSONB column describing
-- the auth method (today: `{"type":"oauth2_pkce", "auth_url": ..., ...}`
-- — no secrets, just endpoints + client_id + scopes).
--
-- The new `mcp_oauth_tokens` table holds short-lived access tokens and
-- (longer-lived) refresh tokens, partitioned by tenant. RLS enforces
-- per-tenant isolation: tokens belonging to tenant A are invisible to
-- tenant B even if the underlying MCP server is system-wide.
--
-- Encryption-at-rest of refresh_token is intentionally deferred to a
-- separate hardening PR — see docs/runbooks/outbound-mcp-oauth.md.

ALTER TABLE mcp_servers
    ADD COLUMN IF NOT EXISTS auth JSONB;

CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
    id            TEXT PRIMARY KEY,
    server_id     TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    tenant_id     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    access_token  TEXT NOT NULL,
    refresh_token TEXT,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (server_id, tenant_id)
);

CREATE INDEX IF NOT EXISTS ix_mcp_oauth_tokens_server_tenant
    ON mcp_oauth_tokens (server_id, tenant_id);

ALTER TABLE mcp_oauth_tokens ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation_mcp_oauth_tokens ON mcp_oauth_tokens
    USING (
        tenant_id = current_setting('app.current_tenant_id', true)
    );
