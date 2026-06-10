-- T3 expert center (docs/plans/2026-06-10-expert-center.md §2.1): named
-- persona compositions with a designated lead. Member ids live in a JSON
-- array (no per-row FK — the API boundary validates members against
-- personas); the lead has a real FK. session_teams keys on session_id so
-- one-team-per-session is DB-enforced, same as session_personas.

CREATE TABLE expert_teams (
    id                     TEXT    PRIMARY KEY,
    name                   TEXT    NOT NULL,
    description            TEXT    NOT NULL DEFAULT '',
    lead_persona_id        TEXT    NOT NULL REFERENCES personas (id) ON DELETE RESTRICT,
    -- JSON array of persona UUIDs; ordered, deduplicated, includes the lead.
    member_persona_ids     TEXT    NOT NULL,
    -- JSON array of pack slugs; display-only selection hints (owner ③A).
    recommended_pack_slugs TEXT,
    created_at             TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    archived               BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (name)
);

CREATE INDEX expert_teams_active_name_idx ON expert_teams (name) WHERE NOT archived;

CREATE TABLE session_teams (
    session_id  TEXT NOT NULL,
    team_id     TEXT NOT NULL REFERENCES expert_teams (id) ON DELETE RESTRICT,
    attached_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (session_id)
);

CREATE INDEX session_teams_team_idx ON session_teams (team_id);
