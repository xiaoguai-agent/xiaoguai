-- v1.5.x: agent-authored skill proposals + a small settings store (SQLite single-user).
--
-- `tenant_settings` keeps its name for repo-layer continuity but is now a
-- single-owner key/value store (no FK to the removed tenants table). JSONB ->
-- TEXT. UNIQUE(tenant_id,name,version) collapses to UNIQUE(name,version).

CREATE TABLE tenant_settings (
    tenant_id   TEXT PRIMARY KEY,
    settings    TEXT NOT NULL DEFAULT '{}',
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE skill_proposals (
    id              TEXT PRIMARY KEY,
    proposed_by     TEXT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT,
    version         TEXT NOT NULL,
    manifest_json   TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','installed')),
    reason          TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    decided_at      TEXT,
    decided_by      TEXT,
    UNIQUE (name, version)
);

CREATE INDEX skill_proposals_status_idx ON skill_proposals (status);
CREATE INDEX skill_proposals_created_idx ON skill_proposals (created_at DESC);
