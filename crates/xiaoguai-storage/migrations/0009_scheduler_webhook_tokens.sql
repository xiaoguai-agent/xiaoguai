-- v0.12.x.1: scheduler webhook tokens (SQLite single-user).
--
-- Backs `/v1/scheduler/webhooks/:route_id`, authenticated via an opaque token
-- in the `X-Xiaoguai-Token` header. A token is bound to one route_id. tenant_id
-- + RLS dropped under the single-user pivot.

CREATE TABLE scheduler_webhook_tokens (
    token           TEXT PRIMARY KEY,
    route_id        TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at    TEXT
);

CREATE INDEX ix_scheduler_webhook_tokens_route
    ON scheduler_webhook_tokens (route_id);
