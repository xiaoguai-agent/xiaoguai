-- packs/it-asset-tracking/migrations/0001_it_assets.sql
--
-- IT Asset Tracking pack — initial schema
--
-- Apply with:  psql $DATABASE_URL -f packs/it-asset-tracking/migrations/0001_it_assets.sql
-- Rollback:    DROP TABLE IF EXISTS
--                  reconciliation_findings,
--                  asset_assignments,
--                  license_terms,
--                  it_assets
--              CASCADE;
--
-- Asset taxonomy
-- ──────────────
--   hardware     : laptop, monitor, phone, peripheral, server, networking
--   software     : saas_seat (per-user SaaS), license (volume/ELA), subscription
--
-- All tables are tenant-scoped (tenant_id) to support multi-tenant deployments.

-- ── it_assets ────────────────────────────────────────────────────────────────
--
-- Single source of truth for every tracked asset.
-- Hardware and software share this table; asset_class + asset_type narrow
-- the kind; the detail JSONB column holds class-specific fields (serial number,
-- MDM enrollment ID, vendor contract number, etc.) without schema sprawl.

CREATE TABLE IF NOT EXISTS it_assets (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL,

    -- Classification
    -- asset_class : 'hardware' | 'software'
    asset_class     TEXT        NOT NULL
                                CHECK (asset_class IN ('hardware', 'software')),
    -- hardware types
    -- asset_type  : 'laptop' | 'monitor' | 'phone' | 'peripheral' | 'server' | 'networking'
    -- software types
    -- asset_type  : 'saas_seat' | 'license' | 'subscription'
    asset_type      TEXT        NOT NULL,

    -- Human-readable identity
    name            TEXT        NOT NULL,      -- e.g. "MacBook Pro 14 M3" or "Slack (Pro)"
    vendor          TEXT        NOT NULL,      -- e.g. "Apple", "Salesforce"
    model           TEXT,                      -- hardware model / software plan tier
    sku             TEXT,                      -- vendor SKU / part number

    -- Hardware-specific identity
    serial_number   TEXT,                      -- NULL for software
    mdm_device_id   TEXT,                      -- Jamf/Intune device record ID
    mdm_source      TEXT,                      -- 'jamf' | 'intune' | NULL

    -- Software-specific identity
    vendor_contract_id  TEXT,                  -- vendor order / contract reference
    total_seats         INT,                   -- NULL for non-seat software
    -- active_seats is a denormalized count kept current by triggers on
    -- asset_assignments; avoids COUNT(*) on every utilization query.
    active_seats        INT     NOT NULL DEFAULT 0,

    -- Lifecycle dates
    purchased_at        DATE,
    warranty_expires_at DATE,                  -- hardware only
    eol_date            DATE,                  -- vendor-announced end-of-life
    license_expires_at  DATE,                  -- software only

    -- Provenance
    -- procurement_source : 'procurement_system' | 'mdm' | 'idp' | 'manual' | 'csv_import'
    procurement_source  TEXT    NOT NULL DEFAULT 'manual',
    procurement_ref     TEXT,                  -- PO number, import batch ID, etc.

    -- Lifecycle status
    -- status : 'in_stock' | 'assigned' | 'in_repair' | 'retired' | 'lost'
    status          TEXT        NOT NULL DEFAULT 'in_stock'
                                CHECK (status IN ('in_stock', 'assigned', 'in_repair', 'retired', 'lost')),

    -- Flexible bag for class-specific fields (e.g. {"screen_size": "14in"})
    detail          JSONB       NOT NULL DEFAULT '{}',

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ia_tenant_idx        ON it_assets (tenant_id);
CREATE INDEX IF NOT EXISTS ia_tenant_class_idx  ON it_assets (tenant_id, asset_class);
CREATE INDEX IF NOT EXISTS ia_tenant_type_idx   ON it_assets (tenant_id, asset_type);
CREATE INDEX IF NOT EXISTS ia_status_idx        ON it_assets (status);
CREATE INDEX IF NOT EXISTS ia_license_exp_idx   ON it_assets (license_expires_at)
    WHERE license_expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS ia_warranty_exp_idx  ON it_assets (warranty_expires_at)
    WHERE warranty_expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS ia_eol_idx           ON it_assets (eol_date)
    WHERE eol_date IS NOT NULL;
CREATE INDEX IF NOT EXISTS ia_mdm_device_idx    ON it_assets (mdm_device_id)
    WHERE mdm_device_id IS NOT NULL;

-- ── asset_assignments ─────────────────────────────────────────────────────────
--
-- Maps assets to the people who hold them.
-- Hardware: one active assignment at a time (enforced by partial unique index).
-- Software: a seat asset can have up to total_seats concurrent active rows.
--
-- Activity tracking (last_active_at, login_count_30d) is populated by
-- inbound events from Okta / MDM usage telemetry. These columns drive the
-- reclaim-recommender watch.

CREATE TABLE IF NOT EXISTS asset_assignments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL,

    asset_id        UUID        NOT NULL REFERENCES it_assets(id) ON DELETE CASCADE,

    -- Person holding the asset (references the employees table from hr-onboarding
    -- if that pack is installed; otherwise a bare UUID is acceptable).
    employee_id     UUID        NOT NULL,
    employee_email  TEXT        NOT NULL,
    employee_name   TEXT        NOT NULL,

    -- Assignment lifecycle
    assigned_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    returned_at     TIMESTAMPTZ,                         -- NULL = still active

    -- Activity signals (refreshed by inbound event handlers)
    last_active_at  TIMESTAMPTZ,          -- last confirmed login / device check-in
    login_count_30d INT         NOT NULL DEFAULT 0,      -- Okta app logins, last 30 days
    mdm_checkin_at  TIMESTAMPTZ,          -- last MDM device check-in (hardware)

    -- Leave / exclusion reason — if set, reclaim-recommender skips this row
    -- even when idle_days_threshold is exceeded.
    -- Populated by inbound Okta HR-sync or manual override.
    -- Values: NULL | 'parental_leave' | 'sabbatical' | 'long_term_sick'
    --         | 'project_hold' | 'admin_override'
    reclaim_exclusion_reason    TEXT,
    reclaim_exclusion_until     DATE,     -- exclusion expires on this date

    -- Provenance: which inbound adapter created / last updated this row
    source          TEXT        NOT NULL DEFAULT 'manual',

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Only one active assignment per hardware asset at a time.
CREATE UNIQUE INDEX IF NOT EXISTS aa_hardware_active_uniq
    ON asset_assignments (asset_id)
    WHERE returned_at IS NULL
      AND (SELECT asset_class FROM it_assets WHERE id = asset_id) = 'hardware';

CREATE INDEX IF NOT EXISTS aa_tenant_idx        ON asset_assignments (tenant_id);
CREATE INDEX IF NOT EXISTS aa_asset_idx         ON asset_assignments (asset_id);
CREATE INDEX IF NOT EXISTS aa_employee_idx      ON asset_assignments (employee_id);
CREATE INDEX IF NOT EXISTS aa_active_idx        ON asset_assignments (tenant_id, returned_at)
    WHERE returned_at IS NULL;
CREATE INDEX IF NOT EXISTS aa_last_active_idx   ON asset_assignments (last_active_at)
    WHERE returned_at IS NULL;
CREATE INDEX IF NOT EXISTS aa_exclusion_idx     ON asset_assignments (reclaim_exclusion_reason)
    WHERE reclaim_exclusion_reason IS NOT NULL;

-- ── license_terms ─────────────────────────────────────────────────────────────
--
-- Contractual terms for software assets.  Separate from it_assets so that
-- a single SaaS product can have overlapping contract periods (e.g. a
-- mid-year expansion order on top of an annual base).

CREATE TABLE IF NOT EXISTS license_terms (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL,

    asset_id            UUID        NOT NULL REFERENCES it_assets(id) ON DELETE CASCADE,

    contract_id         TEXT        NOT NULL,    -- vendor contract / order number
    vendor              TEXT        NOT NULL,
    product_name        TEXT        NOT NULL,
    tier                TEXT,                    -- plan tier: 'pro' | 'enterprise' | etc.

    -- Seat counts
    contracted_seats    INT         NOT NULL,
    -- Annual cost in USD (total, not per-seat) for true-up calculations
    annual_cost_usd     NUMERIC(14,2),
    cost_per_seat_usd   NUMERIC(10,4) GENERATED ALWAYS AS (
        CASE WHEN contracted_seats > 0
             THEN annual_cost_usd / contracted_seats
             ELSE NULL END
    ) STORED,

    -- Term
    term_start_date     DATE        NOT NULL,
    term_end_date       DATE        NOT NULL,
    auto_renews         BOOLEAN     NOT NULL DEFAULT TRUE,
    renewal_notice_days INT         NOT NULL DEFAULT 30,

    -- True-up tracking
    -- last_true_up_date: date of most recent annual license reconciliation
    last_true_up_date   DATE,
    -- true_up_seats_reported: seats we reported to vendor at last true-up
    true_up_seats_reported INT,

    notes               TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS lt_tenant_idx        ON license_terms (tenant_id);
CREATE INDEX IF NOT EXISTS lt_asset_idx         ON license_terms (asset_id);
CREATE INDEX IF NOT EXISTS lt_term_end_idx      ON license_terms (term_end_date);

-- ── reconciliation_findings ───────────────────────────────────────────────────
--
-- Output table written by the audit-reconciler agent.
-- Each row is a discrepancy found between one of three authoritative sources:
--   procurement_system  (imported via csv-procurement-feed or Snipe-IT)
--   mdm                 (Jamf / Intune device inventory)
--   idp                 (Okta app assignments)
--
-- Finding severity: 'critical' | 'high' | 'medium' | 'info'
-- Finding type examples:
--   ghost_license      — IdP shows active user, no MDM record, no procurement record
--   orphan_mdm_device  — MDM shows enrolled device, no procurement record, no assignment
--   missing_assignment — procurement record exists, no active IdP assignment found
--   seat_overcount     — contracted_seats < active_seats (over-provisioned)
--   expired_license    — license_terms.term_end_date < today, still active in IdP

CREATE TABLE IF NOT EXISTS reconciliation_findings (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL,

    -- Run grouping (one reconciliation run produces many findings)
    run_id          UUID        NOT NULL,
    run_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Classification
    finding_type    TEXT        NOT NULL,
    severity        TEXT        NOT NULL
                                CHECK (severity IN ('critical', 'high', 'medium', 'info')),

    -- What was compared
    asset_id        UUID        REFERENCES it_assets(id) ON DELETE SET NULL,
    asset_name      TEXT,
    asset_class     TEXT,
    asset_type      TEXT,

    -- Sources involved
    present_in      TEXT[]      NOT NULL DEFAULT '{}',  -- e.g. ['idp'] — only in IdP
    absent_in       TEXT[]      NOT NULL DEFAULT '{}',  -- e.g. ['mdm','procurement']

    -- Human-readable description of the discrepancy
    description     TEXT        NOT NULL,
    -- LLM-generated recommended action (populated by audit-reconciler)
    recommended_action TEXT,

    -- Resolution lifecycle
    -- resolution_status : 'open' | 'acknowledged' | 'resolved' | 'accepted_risk'
    resolution_status TEXT       NOT NULL DEFAULT 'open'
                                CHECK (resolution_status IN (
                                    'open', 'acknowledged', 'resolved', 'accepted_risk'
                                )),
    resolved_by     TEXT,
    resolved_at     TIMESTAMPTZ,

    detail          JSONB       NOT NULL DEFAULT '{}',

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS rf_tenant_idx        ON reconciliation_findings (tenant_id);
CREATE INDEX IF NOT EXISTS rf_run_idx           ON reconciliation_findings (run_id);
CREATE INDEX IF NOT EXISTS rf_severity_idx      ON reconciliation_findings (severity);
CREATE INDEX IF NOT EXISTS rf_status_idx        ON reconciliation_findings (resolution_status)
    WHERE resolution_status = 'open';
CREATE INDEX IF NOT EXISTS rf_asset_idx         ON reconciliation_findings (asset_id)
    WHERE asset_id IS NOT NULL;
