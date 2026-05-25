-- packs/hr-onboarding/migrations/0001_employees.sql
--
-- HR Onboarding pack — initial schema
--
-- Apply with: psql $DATABASE_URL -f packs/hr-onboarding/migrations/0001_employees.sql
-- Rollback:   DROP TABLE IF EXISTS scheduled_meetings, onboarding_audit_log, employees;

-- ── employees ────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS employees (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL,
    email       TEXT        NOT NULL UNIQUE,
    manager_id  UUID        REFERENCES employees(id) ON DELETE SET NULL,
    start_date  DATE        NOT NULL,
    -- lifecycle: pending | onboarding | active | offboarded
    status      TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'onboarding', 'active', 'offboarded')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS employees_start_date_idx  ON employees (start_date);
CREATE INDEX IF NOT EXISTS employees_status_idx      ON employees (status);

-- ── onboarding_audit_log ─────────────────────────────────────────────────────
--
-- Written by every worker agent as a side effect.
-- Production Okta / Google Workspace calls write here until real MCP tools
-- are wired; the coordinator reads this table to build the onboarding report.

CREATE TABLE IF NOT EXISTS onboarding_audit_log (
    id            BIGSERIAL   PRIMARY KEY,
    employee_id   UUID        NOT NULL REFERENCES employees(id),
    step_id       TEXT        NOT NULL,   -- matches PlanStep::id
    action        TEXT        NOT NULL,
    detail        JSONB       NOT NULL DEFAULT '{}',
    success       BOOLEAN     NOT NULL DEFAULT TRUE,
    error_msg     TEXT,
    recorded_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS oal_employee_idx ON onboarding_audit_log (employee_id);
CREATE INDEX IF NOT EXISTS oal_step_idx     ON onboarding_audit_log (step_id);

-- ── scheduled_meetings ───────────────────────────────────────────────────────
--
-- Written by the meeting-scheduler worker.
-- In production the Calendar MCP tool would create the real calendar events;
-- this table is the mock side-effect that tests assert on.

CREATE TABLE IF NOT EXISTS scheduled_meetings (
    id           BIGSERIAL   PRIMARY KEY,
    employee_id  UUID        NOT NULL REFERENCES employees(id),
    title        TEXT        NOT NULL,
    attendees    TEXT[]      NOT NULL DEFAULT '{}',
    scheduled_at TIMESTAMPTZ NOT NULL,
    duration_min INT         NOT NULL DEFAULT 30,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sm_employee_idx ON scheduled_meetings (employee_id);
