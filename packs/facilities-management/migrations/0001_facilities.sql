-- packs/facilities-management/migrations/0001_facilities.sql
--
-- Facilities Management pack — initial schema
--
-- Apply:    psql $DATABASE_URL -f packs/facilities-management/migrations/0001_facilities.sql
-- Rollback: DROP TABLE IF EXISTS
--             badge_anomalies, space_assignments, vendors_facilities,
--             work_orders CASCADE;

-- ── work_orders ──────────────────────────────────────────────────────────────
--
-- Central table. Every inbound source (Corrigo webhook, Fexa webhook,
-- manual tenant form) normalises to this schema before the triager runs.

CREATE TYPE wo_category AS ENUM (
    'hvac', 'plumbing', 'electrical', 'janitorial', 'security', 'space'
);

CREATE TYPE wo_priority AS ENUM ('p1', 'p2', 'p3');

CREATE TYPE wo_status AS ENUM (
    'new',           -- just created, awaiting triage
    'triaged',       -- category + priority assigned, vendor not yet dispatched
    'dispatched',    -- vendor notified, work not started
    'in_progress',   -- vendor on site / work begun
    'pending_parts', -- waiting on materials
    'resolved',      -- work complete, awaiting confirmation
    'closed',        -- confirmed complete, SLA outcome recorded
    'cancelled'
);

CREATE TABLE IF NOT EXISTS work_orders (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Source traceability
    source          TEXT        NOT NULL,   -- 'corrigo' | 'fexa' | 'manual' | 'watch'
    external_id     TEXT,                   -- CMMS-native ID (nullable for manual)
    UNIQUE (source, external_id),           -- deduplicate re-deliveries

    -- Location
    building        TEXT        NOT NULL,
    floor           TEXT,
    room            TEXT,

    -- Classification
    category        wo_category NOT NULL,
    priority        wo_priority NOT NULL,
    title           TEXT        NOT NULL,
    description     TEXT        NOT NULL DEFAULT '',

    -- SLA tracking
    -- SLA deadline is computed from created_at + per-priority offset.
    sla_deadline    TIMESTAMPTZ NOT NULL,
    -- Populated when status transitions to 'resolved'.
    resolved_at     TIMESTAMPTZ,
    -- True when resolved_at <= sla_deadline (set at close time).
    met_sla         BOOLEAN,

    -- Vendor assignment
    vendor_id       UUID        REFERENCES vendors_facilities(id) ON DELETE SET NULL,
    dispatched_at   TIMESTAMPTZ,

    -- Requestor (tenant / employee)
    requestor_name  TEXT,
    requestor_email TEXT,
    requestor_phone TEXT,

    -- LLM triage metadata (stored for audit + model improvement)
    triage_model    TEXT,
    triage_rationale TEXT,

    status          wo_status   NOT NULL DEFAULT 'new',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS wo_status_idx       ON work_orders (status);
CREATE INDEX IF NOT EXISTS wo_category_idx     ON work_orders (category);
CREATE INDEX IF NOT EXISTS wo_priority_idx     ON work_orders (priority);
CREATE INDEX IF NOT EXISTS wo_sla_deadline_idx ON work_orders (sla_deadline);
CREATE INDEX IF NOT EXISTS wo_vendor_idx       ON work_orders (vendor_id);
CREATE INDEX IF NOT EXISTS wo_created_idx      ON work_orders (created_at);

-- Auto-update updated_at on any row change.
CREATE OR REPLACE FUNCTION facilities_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN NEW.updated_at = now(); RETURN NEW; END;
$$;

CREATE TRIGGER wo_updated_at
    BEFORE UPDATE ON work_orders
    FOR EACH ROW EXECUTE FUNCTION facilities_set_updated_at();

-- ── vendors_facilities ───────────────────────────────────────────────────────
--
-- Approved vendor roster. The work-order-triager queries this table as part
-- of best-vendor-match scoring (see agents/work-order-triager.yaml).

CREATE TABLE IF NOT EXISTS vendors_facilities (
    id                  UUID    PRIMARY KEY DEFAULT gen_random_uuid(),

    name                TEXT    NOT NULL UNIQUE,
    contact_name        TEXT,
    contact_email       TEXT    NOT NULL,
    contact_phone       TEXT,
    dispatch_email      TEXT    NOT NULL,   -- address used for auto-dispatch

    -- Capabilities: which categories this vendor handles.
    -- Stored as an array; query with @> operator.
    categories          wo_category[]  NOT NULL DEFAULT '{}',

    -- Performance metrics (updated by outcome-telemetry at work-order close).
    avg_resolution_hours  NUMERIC(6,2),     -- rolling 90-day average
    sla_compliance_rate   NUMERIC(5,4),     -- 0.0–1.0, rolling 90-day
    open_order_count      INT NOT NULL DEFAULT 0,   -- current active WOs

    -- Compliance documents
    workers_comp_expiry   DATE,
    coi_expiry            DATE,             -- Certificate of Insurance
    license_expiry        DATE,

    active              BOOLEAN NOT NULL DEFAULT true,
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS vf_categories_idx ON vendors_facilities USING GIN (categories);
CREATE INDEX IF NOT EXISTS vf_active_idx     ON vendors_facilities (active);

CREATE TRIGGER vf_updated_at
    BEFORE UPDATE ON vendors_facilities
    FOR EACH ROW EXECUTE FUNCTION facilities_set_updated_at();

-- ── space_assignments ────────────────────────────────────────────────────────
--
-- Desk / office / lab allocations. Populated from the HR onboarding pack
-- (employee record) and updated by the space-planner agent.
-- Badge data is cross-referenced here for utilisation analysis.

CREATE TABLE IF NOT EXISTS space_assignments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    building        TEXT        NOT NULL,
    floor           TEXT        NOT NULL,
    room            TEXT        NOT NULL,
    desk_id         TEXT,                   -- optional sub-room granularity

    -- NULL = unassigned (available for hot-desking pool)
    assignee_email  TEXT,
    assignee_name   TEXT,

    -- Utilisation computed by space-planner over badge_window_days.
    -- 0.0 = never badged in; 1.0 = badged every working day.
    utilization_rate  NUMERIC(5,4),
    last_computed_at  TIMESTAMPTZ,

    -- Desk pool flag: if true, managed as shared hot-desk.
    is_hot_desk     BOOLEAN NOT NULL DEFAULT false,

    effective_from  DATE NOT NULL DEFAULT CURRENT_DATE,
    effective_until DATE,                   -- NULL = open-ended

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sa_building_floor_idx ON space_assignments (building, floor);
CREATE INDEX IF NOT EXISTS sa_assignee_idx       ON space_assignments (assignee_email);
CREATE INDEX IF NOT EXISTS sa_utilization_idx    ON space_assignments (utilization_rate);

CREATE TRIGGER sa_updated_at
    BEFORE UPDATE ON space_assignments
    FOR EACH ROW EXECUTE FUNCTION facilities_set_updated_at();

-- ── badge_anomalies ──────────────────────────────────────────────────────────
--
-- Populated by the badge-system-poll inbound source and the incident-classifier
-- agent. Tracks unexpected badge events (after-hours access, tailgating
-- signals, access-denied spikes) for security work-order correlation.

CREATE TABLE IF NOT EXISTS badge_anomalies (
    id              BIGSERIAL   PRIMARY KEY,

    building        TEXT        NOT NULL,
    door_id         TEXT        NOT NULL,
    event_type      TEXT        NOT NULL,
    -- 'after_hours_access' | 'access_denied_spike' | 'tailgate_signal'
    -- | 'propped_door' | 'unknown_credential'

    badge_holder    TEXT,                   -- NULL for unknown credentials
    occurred_at     TIMESTAMPTZ NOT NULL,

    -- If this anomaly prompted a security work order, link it.
    work_order_id   UUID        REFERENCES work_orders(id) ON DELETE SET NULL,

    raw_payload     JSONB       NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ba_building_idx     ON badge_anomalies (building, occurred_at DESC);
CREATE INDEX IF NOT EXISTS ba_event_type_idx   ON badge_anomalies (event_type);
CREATE INDEX IF NOT EXISTS ba_work_order_idx   ON badge_anomalies (work_order_id);
