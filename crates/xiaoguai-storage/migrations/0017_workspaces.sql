-- migration 0017: workspaces — organisational grouping below the (now removed)
-- tenant, above sessions/boards (SQLite single-user).
--
-- Under DEC-033 there is one implicit owner, so the per-tenant default-workspace
-- seed + back-fill (and the Postgres DO/PLpgSQL blocks, gen_random_uuid, md5
-- casts) are all dropped — a fresh database has no rows to migrate. Workspaces
-- remain as an optional grouping a single user may still create.

CREATE TABLE workspaces (
    id          TEXT        PRIMARY KEY,
    name        TEXT        NOT NULL,
    archived    BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (name)
);

-- Nullable workspace_id FK on each groupable table (NULL = no workspace).
ALTER TABLE sessions
    ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX sessions_workspace_idx ON sessions (workspace_id);

ALTER TABLE agent_outcomes
    ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX agent_outcomes_workspace_idx ON agent_outcomes (workspace_id);

ALTER TABLE installed_skill_packs
    ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX installed_skill_packs_workspace_idx ON installed_skill_packs (workspace_id);

ALTER TABLE hotl_policies
    ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX hotl_policies_workspace_idx ON hotl_policies (workspace_id);
