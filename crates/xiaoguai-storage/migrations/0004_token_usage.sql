-- LLM token usage ledger (SQLite single-user). tenant_id + RLS dropped.
-- Token counts are NULL when the upstream provider doesn't expose them.

CREATE TABLE token_usage (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                  TEXT NOT NULL DEFAULT (datetime('now')),
    user_id             TEXT,
    session_id          TEXT,
    provider_id         TEXT NOT NULL,
    model               TEXT NOT NULL,
    prompt_tokens       INTEGER,
    completion_tokens   INTEGER,
    total_tokens        INTEGER,
    request_id          TEXT
);

CREATE INDEX ix_token_usage_ts ON token_usage (ts);
CREATE INDEX ix_token_usage_provider_ts ON token_usage (provider_id, ts);
