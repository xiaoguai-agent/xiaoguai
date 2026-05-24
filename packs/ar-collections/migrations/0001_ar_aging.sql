-- AR Collections Pack — migration 0001
-- Creates the core accounts-receivable aging table used by all watches,
-- anomaly detectors, and dunning agents in this pack.
--
-- Tenant isolation: every row is scoped to a tenant_id so the pack
-- works in multi-tenant deployments without data leakage.

CREATE TABLE IF NOT EXISTS ar_aging (
    id          TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id   TEXT        NOT NULL,
    customer_id TEXT        NOT NULL,
    invoice_id  TEXT        NOT NULL UNIQUE,
    amount      NUMERIC(18, 4) NOT NULL CHECK (amount >= 0),
    due_date    TIMESTAMPTZ NOT NULL,
    paid_at     TIMESTAMPTZ,               -- NULL = still outstanding
    currency    TEXT        NOT NULL DEFAULT 'USD',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fast overdue look-ups (the watch query filters on paid_at IS NULL + due_date)
CREATE INDEX IF NOT EXISTS ar_aging_tenant_due
    ON ar_aging (tenant_id, due_date)
    WHERE paid_at IS NULL;

-- Customer roll-up for the dunning agent
CREATE INDEX IF NOT EXISTS ar_aging_customer
    ON ar_aging (tenant_id, customer_id);

-- Trigger to keep updated_at current
CREATE OR REPLACE FUNCTION ar_aging_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER ar_aging_updated_at
BEFORE UPDATE ON ar_aging
FOR EACH ROW EXECUTE FUNCTION ar_aging_set_updated_at();

-- Dunning log: records every draft/send action (outcome telemetry for F3)
CREATE TABLE IF NOT EXISTS ar_dunning_log (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    customer_id     TEXT        NOT NULL,
    invoice_ids     TEXT[]      NOT NULL,
    tier            TEXT        NOT NULL CHECK (tier IN ('1st', '2nd', 'final')),
    draft_body      TEXT        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending_approval'
                                CHECK (status IN ('pending_approval', 'approved', 'rejected', 'sent')),
    approved_by     TEXT,
    sent_at         TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ar_dunning_log_customer
    ON ar_dunning_log (tenant_id, customer_id, created_at DESC);

CREATE OR REPLACE TRIGGER ar_dunning_log_updated_at
BEFORE UPDATE ON ar_dunning_log
FOR EACH ROW EXECUTE FUNCTION ar_aging_set_updated_at();
