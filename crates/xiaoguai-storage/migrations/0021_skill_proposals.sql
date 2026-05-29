-- v1.5.x: agent-authored skill proposals + minimal per-tenant settings store.
--
-- `tenant_settings`: tiny JSONB store keyed by tenant_id. Holds opt-in
-- flags like `allow_skill_authoring`. Free-form on purpose so we don't
-- need a migration every time a new opt-in flag lands.
--
-- `skill_proposals`: one row per agent-emitted draft. States:
--   pending → approved → installed   (admin approves; YAML written)
--   pending → rejected               (admin rejects; reason recorded)
-- The `decided_at` + `decided_by` columns capture the human (or system)
-- decision. `manifest_json` holds the full validated manifest as JSONB
-- so we can re-render the YAML on demand and round-trip parse.
--
-- UNIQUE (tenant_id, name, version) prevents the same agent from
-- submitting two identical drafts; a new version requires bumping the
-- version field. Admins delete-and-resubmit if they want to redo a
-- proposal under the same coordinates.

CREATE TABLE tenant_settings (
    tenant_id   TEXT PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    settings    JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE skill_proposals (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    proposed_by     TEXT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT,
    version         TEXT NOT NULL,
    manifest_json   JSONB NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','installed')),
    reason          TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at      TIMESTAMPTZ,
    decided_by      TEXT,
    UNIQUE (tenant_id, name, version)
);

CREATE INDEX skill_proposals_tenant_status_idx
    ON skill_proposals (tenant_id, status);

CREATE INDEX skill_proposals_tenant_created_idx
    ON skill_proposals (tenant_id, created_at DESC);
