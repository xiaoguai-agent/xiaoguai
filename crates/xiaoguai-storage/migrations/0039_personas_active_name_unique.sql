-- Mirror of #283 / migration 0035 (which fixed expert_teams), now for personas.
--
-- The 0016 table-level UNIQUE(name) spans archived rows, so archiving a persona
-- "X" permanently blocks reusing the name "X" for a new active persona — and it
-- diverges from InMemoryPersonaRepository, which only ever checks ACTIVE names.
-- Replace it with a PARTIAL UNIQUE index over non-archived rows so an archived
-- "X" frees the name for a new active "X", while two active personas still can't
-- share a name. Safe on existing data: the old UNIQUE(name) was STRICTER (no two
-- rows could share a name at all), so no current rows can collide under the
-- relaxed active-only rule.
--
-- SQLite can't drop a table-level constraint in place, so rebuild the table.
-- `defer_foreign_keys=ON` lets the DROP + rename run inside this migration's
-- (sqlx-wrapped) transaction despite session_personas.persona_id and
-- expert_teams.lead_persona_id FKs -> personas: both are re-checked at COMMIT,
-- by which point the rebuilt table holds the same ids. It auto-resets at the
-- next commit, so no explicit BEGIN/COMMIT here (sqlx already wraps the migration).
--
-- Columns carried: 0016 base + `tags` (added by 0025). Keep this INSERT in sync
-- if a later migration ALTERs personas.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE personas_rebuilt (
    id              TEXT        PRIMARY KEY,
    name            TEXT        NOT NULL,
    system_prompt   TEXT        NOT NULL DEFAULT '',
    default_model   TEXT,
    tool_allowlist  TEXT,
    escalation_tier TEXT,
    created_at      TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    archived        BOOLEAN     NOT NULL DEFAULT FALSE,
    -- carried from 0025 (S9-7 persona role tags)
    tags            TEXT
);

INSERT INTO personas_rebuilt
    (id, name, system_prompt, default_model, tool_allowlist,
     escalation_tier, created_at, archived, tags)
SELECT id, name, system_prompt, default_model, tool_allowlist,
       escalation_tier, created_at, archived, tags
FROM personas;

DROP TABLE personas;
ALTER TABLE personas_rebuilt RENAME TO personas;

-- Was a NON-unique index in 0016; recreate it UNIQUE so the name uniqueness now
-- applies to ACTIVE personas only (archived names are reusable — #283 pattern).
CREATE UNIQUE INDEX personas_active_name_idx ON personas (name) WHERE NOT archived;
