-- packs/privacy-dsar/migrations/0001_dsar_state.sql
--
-- Privacy DSAR pack — initial schema
--
-- Apply with: psql $DATABASE_URL -f packs/privacy-dsar/migrations/0001_dsar_state.sql
-- Rollback:   DROP TABLE IF EXISTS dsar_disclosures, dsar_data_inventory, dsar_requests;
--
-- Design notes
-- ------------
-- Three tables:
--   dsar_requests       — one row per DSAR, tracks full lifecycle status
--   dsar_data_inventory — operator-maintained per-system PII record findings
--                         (populated during scope-mapping phase)
--   dsar_disclosures    — immutable audit trail of what was exported / erased /
--                         submitted; WORM-style (rows are inserted, never updated)
--
-- Legal-hold awareness
-- --------------------
-- Erasure-cascade-recommender reads the operator-maintained legal_holds table
-- (configured via config.legal_hold.table). Records flagged is_legal_hold = TRUE
-- are emitted as EXCLUDED in the erasure plan rather than included. The pack
-- never deletes held records and documents the exclusion in dsar_requests.erasure_plan_jsonb.

-- ── dsar_requests ─────────────────────────────────────────────────────────────
--
-- One row per DSAR. Status transitions (enforced by pack agents):
--
--   intake_received
--     → identity_pending        (identity-verifier emits verification instructions)
--     → identity_verified       (operator marks verified; scope-mapper activates)
--     → scope_mapped            (scope-mapper completes inventory query)
--     → evidence_assembled      (evidence-gatherer produces export package)
--     → erasure_advisory_ready  (erasure-cascade-recommender finished; type=erasure only)
--     → hotl_pending            (attestation-writer halted; awaiting DPO approval)
--     → hotl_approved           (DPO approved; exfil or erasure advisory sent to operator)
--     → completed               (attestation written; regulator file closed)
--     → rejected                (identity verification failed or request withdrawn)
--     → overdue                 (deadline_at passed without completion)

CREATE TABLE IF NOT EXISTS dsar_requests (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID        NOT NULL,

    -- Who is making the request and on what basis
    requester_name          TEXT        NOT NULL,
    requester_email         TEXT        NOT NULL,
    -- Relationship determines verification risk tier
    -- values: data_subject | authorized_agent | regulator | legal_counsel
    requester_relationship  TEXT        NOT NULL DEFAULT 'data_subject'
                            CHECK (requester_relationship IN (
                                'data_subject', 'authorized_agent', 'regulator', 'legal_counsel'
                            )),

    -- DSAR type per GDPR Arts. 15–22 and CCPA equivalents
    -- access       GDPR Art.15 / CCPA §1798.100 — right to know
    -- erasure      GDPR Art.17 / CCPA §1798.105 — right to delete
    -- portability  GDPR Art.20 / CCPA §1798.110 — right to data portability
    -- rectification GDPR Art.16 — right to correct inaccurate data
    -- objection    GDPR Art.21 — right to object to processing
    request_type            TEXT        NOT NULL
                            CHECK (request_type IN (
                                'access', 'erasure', 'portability',
                                'rectification', 'objection'
                            )),

    -- Inbound source for routing / audit
    -- values: privacy_portal | email_dpo | regulator_forwarded | csv_batch
    inbound_source          TEXT        NOT NULL DEFAULT 'privacy_portal'
                            CHECK (inbound_source IN (
                                'privacy_portal', 'email_dpo',
                                'regulator_forwarded', 'csv_batch'
                            )),

    -- Lifecycle status (see transition diagram above)
    status                  TEXT        NOT NULL DEFAULT 'intake_received'
                            CHECK (status IN (
                                'intake_received', 'identity_pending', 'identity_verified',
                                'scope_mapped', 'evidence_assembled', 'erasure_advisory_ready',
                                'hotl_pending', 'hotl_approved', 'completed',
                                'rejected', 'overdue'
                            )),

    -- Regulatory deadline (operator may override default_days from config)
    deadline_at             TIMESTAMPTZ NOT NULL,

    -- Free-text description of the request (from intake form or email body)
    request_description     TEXT,

    -- Identity verification
    identity_verification_method  TEXT,           -- e.g. 'email_link', 'video_call', 'notarized_id'
    identity_verified_at          TIMESTAMPTZ,    -- set by operator after verification exchange
    identity_verifier_agent_id    TEXT,           -- agent run that proposed the method

    -- Scope mapping results
    systems_in_scope        TEXT[]      NOT NULL DEFAULT '{}',  -- system IDs from connected_systems
    scope_mapped_at         TIMESTAMPTZ,

    -- Evidence / export package
    export_package_s3_key   TEXT,           -- S3 key (not presigned URL — URL is ephemeral)
    export_assembled_at     TIMESTAMPTZ,

    -- Erasure advisory (type=erasure only)
    -- JSONB document containing:
    --   { "ordered_deletions": [...], "excluded_legal_hold": [...],
    --     "excluded_retention_obligation": [...], "advisory_notes": "..." }
    erasure_plan_jsonb      JSONB,
    erasure_advisory_ready_at TIMESTAMPTZ,

    -- HotL approval
    hotl_approved_by        TEXT,           -- DPO identifier
    hotl_approved_at        TIMESTAMPTZ,
    hotl_rejection_reason   TEXT,           -- set if DPO rejects

    -- Attestation
    attestation_doc_s3_key  TEXT,           -- immutable regulator attestation document
    completed_at            TIMESTAMPTZ,

    -- Rejection / withdrawal
    rejection_reason        TEXT,

    -- Metadata
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dsar_requests_tenant_idx    ON dsar_requests (tenant_id);
CREATE INDEX IF NOT EXISTS dsar_requests_status_idx    ON dsar_requests (status);
CREATE INDEX IF NOT EXISTS dsar_requests_deadline_idx  ON dsar_requests (deadline_at);
CREATE INDEX IF NOT EXISTS dsar_requests_email_idx     ON dsar_requests (requester_email);
CREATE INDEX IF NOT EXISTS dsar_requests_type_idx      ON dsar_requests (request_type);

-- ── dsar_data_inventory ──────────────────────────────────────────────────────
--
-- Per-system findings from the scope-mapping phase.
-- One row per (dsar_request, system) pair.
-- Populated by the scope-mapper agent querying the operator's connected_systems.
--
-- HONEST CAVEAT: The completeness of these findings depends entirely on the
-- accuracy of the operator's data inventory. Systems not registered in
-- config.connected_systems are NOT queried and NOT represented here.

CREATE TABLE IF NOT EXISTS dsar_data_inventory (
    id                  BIGSERIAL   PRIMARY KEY,
    dsar_request_id     UUID        NOT NULL REFERENCES dsar_requests(id) ON DELETE CASCADE,
    tenant_id           UUID        NOT NULL,

    -- System identifier (matches config.connected_systems[].id)
    system_id           TEXT        NOT NULL,
    system_label        TEXT        NOT NULL,

    -- Whether any records matching the requester were found
    records_found       BOOLEAN     NOT NULL DEFAULT FALSE,
    record_count        INT,                    -- null if system did not report count

    -- Snapshot of matching record identifiers (for evidence assembly)
    -- Stored as JSONB to handle heterogeneous primary-key types across systems.
    -- Example: {"user_id": "u-123", "table": "users"}
    record_refs         JSONB       NOT NULL DEFAULT '[]',

    -- Categories of personal data found (free-form, operator-provided labels)
    -- Example: ["contact_info", "transaction_history", "behavioral_logs"]
    data_categories     TEXT[]      NOT NULL DEFAULT '{}',

    -- Legal basis for processing (GDPR Art.6 / CCPA context)
    legal_basis         TEXT,

    -- Retention schedule (informs erasure cascade)
    retention_policy    TEXT,

    -- Whether any records are under legal hold (prevents erasure)
    has_legal_hold      BOOLEAN     NOT NULL DEFAULT FALSE,

    -- Agent run that produced this row
    mapped_by_agent_id  TEXT,
    mapped_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dsar_inv_request_idx ON dsar_data_inventory (dsar_request_id);
CREATE INDEX IF NOT EXISTS dsar_inv_tenant_idx  ON dsar_data_inventory (tenant_id);
CREATE INDEX IF NOT EXISTS dsar_inv_system_idx  ON dsar_data_inventory (system_id);

-- ── dsar_disclosures ──────────────────────────────────────────────────────────
--
-- Immutable WORM-style audit trail of every significant action taken on a
-- DSAR. Rows are only ever INSERTed — never UPDATEd or DELETEd.
-- A PostgreSQL row-level security policy (to be applied by the operator's
-- DBA) should REVOKE DELETE, UPDATE on this table.
--
-- Disclosure types:
--   export_sent           — data package delivered to requester
--   erasure_advisory      — erasure plan delivered to operator
--   regulator_submitted   — response filed with regulator
--   hotl_approved         — DPO approved the request
--   hotl_rejected         — DPO rejected the request
--   identity_verified     — identity verification confirmed
--   status_change         — lifecycle status transition recorded
--   attestation_issued    — legal attestation document created

CREATE TABLE IF NOT EXISTS dsar_disclosures (
    id                  BIGSERIAL   PRIMARY KEY,
    dsar_request_id     UUID        NOT NULL REFERENCES dsar_requests(id),
    tenant_id           UUID        NOT NULL,

    -- Disclosure event type
    disclosure_type     TEXT        NOT NULL
                        CHECK (disclosure_type IN (
                            'export_sent', 'erasure_advisory', 'regulator_submitted',
                            'hotl_approved', 'hotl_rejected', 'identity_verified',
                            'status_change', 'attestation_issued'
                        )),

    -- For export_sent: who received the data and how
    recipient_identity  TEXT,           -- requester email or regulator identifier
    delivery_method     TEXT,           -- 's3_presigned_url' | 'regulator_portal' | 'email'
    delivery_reference  TEXT,           -- S3 key, portal submission ID, or message ID

    -- Status transition (for status_change type)
    from_status         TEXT,
    to_status           TEXT,

    -- Actor performing or approving the action
    actor_type          TEXT        NOT NULL DEFAULT 'agent'
                        CHECK (actor_type IN ('agent', 'dpo', 'operator', 'system')),
    actor_id            TEXT,           -- agent run ID or human identifier

    -- Free-form detail (JSON) — e.g. systems affected, record counts, S3 keys
    detail              JSONB       NOT NULL DEFAULT '{}',

    -- Immutable timestamp — never updated
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dsar_disc_request_idx ON dsar_disclosures (dsar_request_id);
CREATE INDEX IF NOT EXISTS dsar_disc_tenant_idx  ON dsar_disclosures (tenant_id);
CREATE INDEX IF NOT EXISTS dsar_disc_type_idx    ON dsar_disclosures (disclosure_type);
CREATE INDEX IF NOT EXISTS dsar_disc_time_idx    ON dsar_disclosures (recorded_at);

-- ── Trigger: updated_at maintenance ──────────────────────────────────────────

CREATE OR REPLACE FUNCTION dsar_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER dsar_requests_updated_at
    BEFORE UPDATE ON dsar_requests
    FOR EACH ROW EXECUTE FUNCTION dsar_set_updated_at();
