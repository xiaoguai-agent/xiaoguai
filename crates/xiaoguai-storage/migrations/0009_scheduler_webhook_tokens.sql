-- v0.12.x.1: per-tenant scheduler webhook tokens.
--
-- Today `/v1/admin/scheduler/webhooks/:route_id` is admin-bearer-gated;
-- external integrators (GitHub push, Slack events, internal cron from a
-- different tenant) can't reach it without an admin token. This table
-- backs a sibling route at `/v1/scheduler/webhooks/:route_id` (note: NOT
-- under `/admin`) that authenticates via a per-tenant opaque token in
-- the `X-Xiaoguai-Token` request header.
--
-- A token is bound to exactly one (tenant, route_id) pair. Admins
-- manage them via the v0.12.x.1 `/v1/admin/scheduler/tokens` endpoints.
-- Tokens are opaque random strings (the API returns them once on
-- creation; rotation is "delete + create").
--
-- `last_used_at` is best-effort — the route updates it on every
-- successful push so admins can spot stale tokens. Update failures do
-- NOT block the push (the audit row is the source of truth).
--
-- RLS: scoped to the owning tenant via the same GUC pattern the rest of
-- the workspace uses. Admin-side CRUD bypasses RLS by running with the
-- container superuser; the route handler must set
-- `app.current_tenant_id` after validating the token so that side-effects
-- (e.g. a future audit row written from the same tx) inherit the right
-- tenant.

CREATE TABLE scheduler_webhook_tokens (
    token           TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    route_id        TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at    TIMESTAMPTZ
);

CREATE INDEX ix_scheduler_webhook_tokens_tenant
    ON scheduler_webhook_tokens (tenant_id, created_at DESC);

CREATE INDEX ix_scheduler_webhook_tokens_route
    ON scheduler_webhook_tokens (route_id);

ALTER TABLE scheduler_webhook_tokens ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation_scheduler_webhook_tokens
    ON scheduler_webhook_tokens
    USING (
        tenant_id = current_setting('app.current_tenant_id', true)
    );
