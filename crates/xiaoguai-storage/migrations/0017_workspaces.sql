-- migration 0017: workspaces — organisational grouping above sessions/boards,
-- below tenant (ADR-0019 Hermes inspiration, v1.3.x).
--
-- A workspace lets a tenant partition their boards, sessions, skill-packs,
-- HOTL policies, and outcomes into named groups (teams, products, projects).
-- Every tenant gets exactly one default workspace; all existing rows are
-- reassigned to it so the migration is backward-compatible.
--
-- Hierarchy: tenant ⊇ workspace ⊇ board | session | installed_skill_pack |
--                                          hotl_policy | agent_outcome

-- ---------------------------------------------------------------------------
-- workspaces
-- ---------------------------------------------------------------------------

CREATE TABLE workspaces (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL,
    name        TEXT        NOT NULL,
    archived    BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, name)
);

CREATE INDEX workspaces_tenant_idx
    ON workspaces (tenant_id);

-- ---------------------------------------------------------------------------
-- Add workspace_id FK to all scoped tables (nullable; NULL = use default)
-- ---------------------------------------------------------------------------

-- sessions
ALTER TABLE sessions
    ADD COLUMN workspace_id UUID REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX sessions_workspace_idx ON sessions (workspace_id);

-- agent_outcomes (migration 0012)
ALTER TABLE agent_outcomes
    ADD COLUMN workspace_id UUID REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX agent_outcomes_workspace_idx ON agent_outcomes (workspace_id);

-- installed_skill_packs (migration 0015)
ALTER TABLE installed_skill_packs
    ADD COLUMN workspace_id UUID REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX installed_skill_packs_workspace_idx ON installed_skill_packs (workspace_id);

-- hotl_policies (migration 0011)
ALTER TABLE hotl_policies
    ADD COLUMN workspace_id UUID REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX hotl_policies_workspace_idx ON hotl_policies (workspace_id);

-- boards (migration 0016 — additive only; 0016 may land on a parallel branch)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_name = 'boards') THEN
        ALTER TABLE boards
            ADD COLUMN IF NOT EXISTS workspace_id UUID
            REFERENCES workspaces (id) ON DELETE SET NULL;
        CREATE INDEX IF NOT EXISTS boards_workspace_idx
            ON boards (workspace_id);
    END IF;
END $$;

-- tasks (migration 0016 — additive only; conditioned on boards existing)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_name = 'tasks') THEN
        ALTER TABLE tasks
            ADD COLUMN IF NOT EXISTS workspace_id UUID
            REFERENCES workspaces (id) ON DELETE SET NULL;
        CREATE INDEX IF NOT EXISTS tasks_workspace_idx
            ON tasks (workspace_id);
    END IF;
END $$;

-- ---------------------------------------------------------------------------
-- Seed: for each existing tenant create a default workspace and reassign
-- all existing rows to it. The INSERT uses the tenant's id as a deterministic
-- UUID seed so re-runs are idempotent (gen_random_uuid would break re-runs).
-- ---------------------------------------------------------------------------

-- 1. Create one default workspace per tenant (named 'default').
--    We need a stable UUID: derive it from the tenant id via md5 + UUID v3
--    encoding so the row is exactly the same if the migration is somehow
--    applied twice (idempotent via ON CONFLICT DO NOTHING).
INSERT INTO workspaces (id, tenant_id, name, archived, created_at)
SELECT
    -- Deterministic UUID: md5(tenant_id || ':default') cast to UUID.
    CAST(md5(id::text || ':default') AS UUID),
    CAST(id AS UUID),
    'default',
    FALSE,
    NOW()
FROM tenants
ON CONFLICT (id) DO NOTHING;

-- 2. Back-fill workspace_id on all existing rows to point at their tenant's
--    default workspace. Only rows that are NULL (i.e. pre-migration) are
--    updated; a WHERE clause makes the UPDATE safe to re-run.

UPDATE sessions s
SET workspace_id = CAST(md5(s.tenant_id || ':default') AS UUID)
WHERE s.workspace_id IS NULL;

UPDATE agent_outcomes ao
SET workspace_id = CAST(md5(ao.tenant_id::text || ':default') AS UUID)
WHERE ao.workspace_id IS NULL;

UPDATE installed_skill_packs isp
SET workspace_id = CAST(md5(isp.tenant_id::text || ':default') AS UUID)
WHERE isp.workspace_id IS NULL;

UPDATE hotl_policies hp
SET workspace_id = CAST(md5(hp.tenant_id::text || ':default') AS UUID)
WHERE hp.workspace_id IS NULL;
