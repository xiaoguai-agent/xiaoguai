-- LLM token usage ledger.
--
-- One row per `chat_stream` call (recorded when the stream reaches
-- `done: true`). Token counts are `NULL` when the upstream provider doesn't
-- expose them; downstream cost-attribution code must tolerate this.

CREATE TABLE token_usage (
    id                  BIGSERIAL PRIMARY KEY,
    ts                  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    tenant_id           TEXT NOT NULL,
    user_id             TEXT,
    session_id          TEXT,
    provider_id         TEXT NOT NULL,
    model               TEXT NOT NULL,
    prompt_tokens       INT,
    completion_tokens   INT,
    total_tokens        INT,
    request_id          TEXT
);

CREATE INDEX ix_token_usage_tenant_ts ON token_usage (tenant_id, ts);
CREATE INDEX ix_token_usage_provider_ts ON token_usage (provider_id, ts);

ALTER TABLE token_usage ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation_token_usage ON token_usage
    USING (tenant_id = current_setting('app.current_tenant_id', true));
