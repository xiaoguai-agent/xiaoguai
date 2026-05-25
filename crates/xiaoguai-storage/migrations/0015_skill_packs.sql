-- v1.2.28: skill marketplace — per-tenant installed skill packs.
--
-- Each row records one pack installed by a tenant operator. The `config`
-- column holds any knob overrides the operator supplied at install time;
-- the schema is free-form JSONB because each pack declares its own knobs
-- and we don't want to add a migration every time a new pack adds a field.
--
-- UNIQUE (tenant_id, pack_slug) prevents double-installs at the DB level;
-- the API layer returns 409 Conflict when it hits this constraint.

CREATE TABLE installed_skill_packs (
    id          UUID        PRIMARY KEY,
    tenant_id   UUID        NOT NULL,
    pack_slug   TEXT        NOT NULL,
    version     TEXT        NOT NULL,
    config      JSONB       NOT NULL DEFAULT '{}'::jsonb,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, pack_slug)
);

CREATE INDEX installed_skill_packs_tenant_idx
    ON installed_skill_packs (tenant_id);
