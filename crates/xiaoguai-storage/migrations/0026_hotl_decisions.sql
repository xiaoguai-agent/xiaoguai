-- v1.8.x sprint-11 (S11-3a.1): HOTL decision-record ledger (SQLite single-user).
-- One row per human decision on an escalated request. UUID -> TEXT; tenant_id dropped.

CREATE TABLE hotl_decisions (
    id               TEXT PRIMARY KEY,
    request_id       TEXT NOT NULL UNIQUE,
    verdict          TEXT NOT NULL CHECK (verdict IN ('allow', 'deny')),
    decided_by       TEXT NOT NULL,
    raised_policy_id TEXT,
    recorded_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX hotl_decisions_recent ON hotl_decisions (recorded_at DESC);
