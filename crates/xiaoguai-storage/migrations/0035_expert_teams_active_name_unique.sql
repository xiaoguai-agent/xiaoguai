-- #283: an archived team's name could never be reused — the 0032 table-level
-- UNIQUE(name) spans archived rows, diverging from the in-memory repository
-- (which only checks ACTIVE teams). Replace it with a PARTIAL UNIQUE index over
-- non-archived rows so an archived "X" frees the name for a new active "X",
-- while two active teams still can't share a name.
--
-- SQLite can't drop a table-level constraint in place, so rebuild the table.
-- `defer_foreign_keys=ON` lets the DROP + rename run inside this migration's
-- (sqlx-wrapped) transaction despite session_teams' FK -> expert_teams and the
-- lead_persona_id FK -> personas: both are re-checked at COMMIT, by which point
-- the rebuilt table holds the same ids. It auto-resets at the next commit, so
-- no explicit BEGIN/COMMIT here (sqlx already wraps the migration).
PRAGMA defer_foreign_keys = ON;

CREATE TABLE expert_teams_rebuilt (
    id                     TEXT    PRIMARY KEY,
    name                   TEXT    NOT NULL,
    description            TEXT    NOT NULL DEFAULT '',
    lead_persona_id        TEXT    NOT NULL REFERENCES personas (id) ON DELETE RESTRICT,
    member_persona_ids     TEXT    NOT NULL,
    recommended_pack_slugs TEXT,
    created_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    archived               BOOLEAN NOT NULL DEFAULT FALSE,
    -- carried from 0034 (T7.1 glossary)
    glossary_md            TEXT
);

INSERT INTO expert_teams_rebuilt
    (id, name, description, lead_persona_id, member_persona_ids,
     recommended_pack_slugs, created_at, archived, glossary_md)
SELECT id, name, description, lead_persona_id, member_persona_ids,
       recommended_pack_slugs, created_at, archived, glossary_md
FROM expert_teams;

DROP TABLE expert_teams;
ALTER TABLE expert_teams_rebuilt RENAME TO expert_teams;

-- Was a NON-unique index in 0032; recreate it UNIQUE so the name uniqueness
-- now applies to ACTIVE teams only.
CREATE UNIQUE INDEX expert_teams_active_name_idx ON expert_teams (name) WHERE NOT archived;
