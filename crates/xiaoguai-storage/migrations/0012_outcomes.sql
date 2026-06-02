-- migration 0012: agent_outcomes — "revenue, not time" outcome telemetry (SQLite).
-- UUIDs -> TEXT; tenant_id dropped; NUMERIC -> REAL; JSONB -> TEXT.

CREATE TABLE agent_outcomes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT,
    agent_name      TEXT NOT NULL,
    -- 'revenue_usd' | 'cost_saved_usd' | 'hours_saved' | 'deals_closed'
    -- | 'tickets_resolved' | 'custom'
    kind            TEXT NOT NULL,
    value           REAL NOT NULL,
    unit            TEXT,           -- 'usd' | 'hours' | 'count'
    description     TEXT,
    attributed_at   TEXT DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    metadata        TEXT DEFAULT '{}'
);

CREATE INDEX outcomes_kind_time ON agent_outcomes (kind, attributed_at);
