-- LLM provider registry.
--
-- A row is either:
--   * tenant-scoped (`tenant_id` non-NULL) — visible only inside that tenant
--   * system-wide  (`tenant_id` NULL)      — visible to every tenant
--
-- Tenants override system defaults via the higher precedence of their own
-- row in `default_for_models`. The router walks `fallback_order` ascending
-- when no explicit/default match exists.

CREATE TABLE llm_providers (
    id                  TEXT PRIMARY KEY,
    tenant_id           TEXT REFERENCES tenants(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    kind                TEXT NOT NULL,
    endpoint            TEXT NOT NULL,
    models              JSONB NOT NULL DEFAULT '[]'::jsonb,
    default_for_models  JSONB NOT NULL DEFAULT '[]'::jsonb,
    fallback_order      INT NOT NULL DEFAULT 100,
    api_key_env         TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Names are unique within a scope. We use COALESCE(tenant_id,'') so the
-- NULL-as-global rows still participate in uniqueness checks.
CREATE UNIQUE INDEX ux_llm_providers_scope_name
    ON llm_providers (COALESCE(tenant_id, ''), name);

CREATE INDEX ix_llm_providers_scope_fallback
    ON llm_providers (COALESCE(tenant_id, ''), fallback_order);

ALTER TABLE llm_providers ENABLE ROW LEVEL SECURITY;

-- Tenant rows visible only to that tenant; global rows visible to all.
CREATE POLICY tenant_or_global_isolation ON llm_providers
    USING (
        tenant_id IS NULL
        OR tenant_id = current_setting('app.current_tenant_id', true)
    );
