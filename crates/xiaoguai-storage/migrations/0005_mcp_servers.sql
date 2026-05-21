-- MCP server manifest registry.
--
-- A row is either:
--   * tenant-scoped (`tenant_id` non-NULL) — visible only inside that tenant
--   * system-wide  (`tenant_id` NULL)      — visible to every tenant
--
-- Secrets policy (mirrors llm_providers): only env-var NAMES are stored
-- (`env_keys`), never the values. The runtime resolves them when spawning
-- the MCP child process.

CREATE TABLE mcp_servers (
    id          TEXT PRIMARY KEY,
    tenant_id   TEXT REFERENCES tenants(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL DEFAULT '0.0.0',
    transport   TEXT NOT NULL,                 -- 'stdio' | 'sse' | 'http'
    command     TEXT,                          -- stdio only
    args        JSONB NOT NULL DEFAULT '[]'::jsonb,
    env_keys    JSONB NOT NULL DEFAULT '[]'::jsonb,
    endpoint    TEXT,                          -- sse / http only
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX ux_mcp_servers_scope_name_version
    ON mcp_servers (COALESCE(tenant_id, ''), name, version);

CREATE INDEX ix_mcp_servers_scope_enabled
    ON mcp_servers (COALESCE(tenant_id, ''), enabled);

ALTER TABLE mcp_servers ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_or_global_isolation_mcp ON mcp_servers
    USING (
        tenant_id IS NULL
        OR tenant_id = current_setting('app.current_tenant_id', true)
    );
