-- v1.3.x: agent persona / role profiles (SQLite single-user).
-- UUID -> TEXT; tenant_id dropped; Postgres text[] tool_allowlist -> TEXT holding
-- a JSON array (NULL = all tools allowed; '[]' = no tools).

CREATE TABLE personas (
    id              TEXT        PRIMARY KEY,
    name            TEXT        NOT NULL,
    system_prompt   TEXT        NOT NULL DEFAULT '',
    default_model   TEXT,
    tool_allowlist  TEXT,
    escalation_tier TEXT,
    created_at      TEXT        NOT NULL DEFAULT (datetime('now')),
    archived        BOOLEAN     NOT NULL DEFAULT FALSE,
    UNIQUE (name)
);

CREATE INDEX personas_active_name_idx ON personas (name) WHERE NOT archived;

CREATE TABLE session_personas (
    session_id  TEXT        NOT NULL,
    persona_id  TEXT        NOT NULL REFERENCES personas (id) ON DELETE RESTRICT,
    attached_at TEXT        NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (session_id)
);

CREATE INDEX session_personas_persona_idx ON session_personas (persona_id);
