-- v1.2.3: Human-on-the-Loop (HOTL) boundary policy (SQLite single-user).
-- UUIDs are TEXT; tenant_id dropped; NUMERIC(10,4) -> REAL.
--
-- `hotl_policies` — one row per scope budget. `hotl_usage_log` — append-only
-- ledger; the enforcer sums `amount` within the rolling window to decide.

CREATE TABLE hotl_policies (
    id              TEXT PRIMARY KEY,
    scope           TEXT NOT NULL,
    window_seconds  INTEGER NOT NULL,
    max_count       INTEGER,
    max_usd         REAL,
    escalate_to     TEXT,
    created_at      TEXT DEFAULT (datetime('now'))
);

CREATE INDEX hotl_policies_scope ON hotl_policies (scope);

CREATE TABLE hotl_usage_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    scope       TEXT NOT NULL,
    amount      REAL NOT NULL,
    escalated   BOOLEAN DEFAULT FALSE,
    occurred_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX hotl_usage_scope_time ON hotl_usage_log (scope, occurred_at);
