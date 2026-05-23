-- v0.10.0: scheduler — ScheduledJob + JobRun.
--
-- Two tables, RLS-isolated, tenant-scoped (or system-wide via NULL).
--
-- `scheduled_jobs.trigger` is JSONB serialized via xiaoguai_scheduler::Trigger
-- (internally-tagged):
--   { "type": "cron",     "expr": "0 0 * * * *" }
--   { "type": "interval", "secs": 3600 }
--
-- `scheduled_jobs.retry_policy` mirrors xiaoguai_scheduler::RetryPolicy:
--   { "max_attempts": 3, "initial_backoff_secs": 30,
--     "multiplier": 2.0, "max_backoff_secs": 3600 }
--
-- Cross-references for the audit-first console (roadmap §5.3):
--   scheduled_job_runs.session_id → sessions.id (when the run produced
--     a chat-style transcript). audit_log carries
--     details->>'run_id' and actor = 'scheduler:<job_id>' so the
--     console can join chat / IM / scheduled runs into one timeline.

CREATE TABLE scheduled_jobs (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT REFERENCES tenants(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    trigger         JSONB NOT NULL,
    payload         JSONB NOT NULL,
    retry_policy    JSONB NOT NULL,
    sinks           JSONB NOT NULL DEFAULT '[]'::jsonb,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    next_fire_at    TIMESTAMPTZ,
    last_fire_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ix_scheduled_jobs_due
    ON scheduled_jobs (enabled, next_fire_at)
    WHERE enabled IS TRUE;

CREATE INDEX ix_scheduled_jobs_tenant
    ON scheduled_jobs (COALESCE(tenant_id, ''));

ALTER TABLE scheduled_jobs ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_or_global_isolation_scheduled_jobs ON scheduled_jobs
    USING (
        tenant_id IS NULL
        OR tenant_id = current_setting('app.current_tenant_id', true)
    );

CREATE TABLE scheduled_job_runs (
    id              BIGSERIAL PRIMARY KEY,
    job_id          TEXT NOT NULL REFERENCES scheduled_jobs(id) ON DELETE CASCADE,
    tenant_id       TEXT,
    status          TEXT NOT NULL,
    attempt         INTEGER NOT NULL DEFAULT 1,
    started_at      TIMESTAMPTZ,
    finished_at     TIMESTAMPTZ,
    session_id      TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    error_message   TEXT,
    output_preview  TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ix_scheduled_job_runs_job
    ON scheduled_job_runs (job_id, created_at DESC);

CREATE INDEX ix_scheduled_job_runs_tenant_status
    ON scheduled_job_runs (COALESCE(tenant_id, ''), status, created_at DESC);

ALTER TABLE scheduled_job_runs ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_or_global_isolation_scheduled_job_runs ON scheduled_job_runs
    USING (
        tenant_id IS NULL
        OR tenant_id = current_setting('app.current_tenant_id', true)
    );
