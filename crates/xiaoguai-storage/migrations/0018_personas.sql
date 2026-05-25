-- v1.3.x: Agent persona / role profiles.
--
-- `personas` — one row per named role profile within a tenant.
--   `name`             — human-readable label (e.g. "Support Bot", "Finance Analyst").
--   `system_prompt`    — text injected as the first system message in every chat turn.
--   `default_model`    — optional model override (e.g. "gpt-4o-mini"). NULL = use
--                        the session / tenant default.
--   `tool_allowlist`   — Postgres text[] of tool names the persona may invoke.
--                        Empty array = no tools allowed. NULL = all tools allowed
--                        (unrestricted). The runtime enforces this at dispatch time.
--   `escalation_tier`  — opaque label (e.g. "L1", "L2", "human") consumed by the
--                        HOTL escalation path. NULL = no escalation tier configured.
--   `archived`         — soft-delete. Archived personas cannot be attached to new
--                        sessions but existing attachments continue to serve.
--
-- `session_personas` — join table: which persona (if any) is active for a session.
--   The FK on `session_id` CASCADE-DELETEs automatically when a session is purged.
--   A session may have at most one active persona at a time; the unique constraint
--   on `session_id` enforces this at the DB level. Replacing the persona detaches
--   the old row and inserts a new one (upsert in the repository layer).

CREATE TABLE personas (
    id              UUID        PRIMARY KEY,
    tenant_id       UUID        NOT NULL,
    name            TEXT        NOT NULL,
    system_prompt   TEXT        NOT NULL DEFAULT '',
    default_model   TEXT,
    tool_allowlist  TEXT[],
    escalation_tier TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived        BOOLEAN     NOT NULL DEFAULT false,
    UNIQUE (tenant_id, name)
);

CREATE INDEX personas_tenant_idx     ON personas (tenant_id);
CREATE INDEX personas_tenant_name_idx ON personas (tenant_id, name) WHERE NOT archived;

CREATE TABLE session_personas (
    session_id  TEXT        NOT NULL,
    persona_id  UUID        NOT NULL REFERENCES personas (id) ON DELETE RESTRICT,
    attached_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id),
    UNIQUE (session_id)
);

CREATE INDEX session_personas_persona_idx ON session_personas (persona_id);
