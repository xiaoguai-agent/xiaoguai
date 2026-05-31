-- v1.10.x sprint-13 (S13-1, DEC-HLD-013/014/016): HotL escalation
-- parent/child split + redaction policies + Casbin hotl:decide scope seed.
--
-- This migration is the long-pole for sprint-13. Three coupled DDL+seed
-- changes ship atomically:
--
-- 1. `hotl_escalations` — new parent table (one row per top-level invocation).
--    Nested gating (DEC-HLD-021 triangle + per-peer HotL) writes one parent
--    plus N children sharing the same `escalation_id` lineage via `parent_id`.
--    Used by `HotlEscalationStore::insert_pending` (S13-2) and the boot
--    replay query in `xiaoguai-core::run_serve` (S13-5).
--
-- 2. `hotl_pending` — new child table for in-flight escalations. Holds the
--    persisted backing for `DecisionRegistry` waiters so an `xiaoguai-api`
--    restart can re-mint oneshot pairs from the indexed scan
--    `WHERE status='pending' AND expires_at > now()`. Column `escalation_id`
--    is the canonical FK (DEC-HLD-016 rename — `request_id` is gone).
--
--    Note: the sprint plan §S13-1 describes a "backfill" of pre-existing
--    v1.9-shape `hotl_pending` rows into the new parent table. In practice
--    migration 0026 only created `hotl_decisions` (a record-of-decision
--    ledger), so the table being "refactored" never shipped — the backfill
--    block below is therefore a NO-OP guarded INSERT that triggers only if
--    a partial pre-0027 schema is present (defensive against branch-merged
--    or hand-rolled deployments). The 1-to-1 invariant per GR-DB-02 is
--    preserved either way.
--
-- 3. `hotl_redaction_policies` — per-tenant JSONPath redaction rules
--    consumed by `xiaoguai-auth::redaction::RedactionRules` (S13-4) before
--    `HotlPending` events are emitted on the SSE channel (S13-6, DEC-HLD-014).
--    Ships empty; admin-ui CRUD lands in sprint-14.
--
-- 4. `casbin_rule` — minimal seed table for the Casbin `hotl:decide` scope
--    (DEC-HLD-016). The compiled-in RBAC policy in
--    `crates/xiaoguai-auth/policies/rbac_policy.csv` currently has no
--    `/v1/hotl/decisions` rule at all (sprint-11/12 route is not Casbin-
--    enforced yet), so there is no path-based "fallback" rule to remove.
--    We still seed a `casbin_rule` table with the scope-based grant so
--    S13-10 can wire the route to a DB-backed Casbin adapter without a
--    second migration; the DELETE below covers any local deploys that
--    hand-rolled the path-based rule before this migration ran.
--
-- Forward-only per GR-DB-02 — no DOWN migration. Rollback is via a
-- follow-up `0028_revert_*.sql` if it becomes necessary.

CREATE TABLE hotl_escalations (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id        UUID NOT NULL,
    session_id       UUID NOT NULL,
    top_level_scope  TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'resolved', 'expired')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    parent_id        UUID REFERENCES hotl_escalations(id) ON DELETE CASCADE
);

CREATE INDEX hotl_escalations_tenant_id_idx ON hotl_escalations (tenant_id);
CREATE INDEX hotl_escalations_session_id_idx ON hotl_escalations (session_id);
CREATE INDEX hotl_escalations_status_idx
    ON hotl_escalations (status)
    WHERE status = 'pending';

ALTER TABLE hotl_escalations ENABLE ROW LEVEL SECURITY;

CREATE POLICY hotl_escalations_tenant_isolation ON hotl_escalations
    USING (tenant_id::text = current_setting('app.current_tenant_id', true));

CREATE TABLE hotl_pending (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    escalation_id   UUID REFERENCES hotl_escalations(id) ON DELETE CASCADE,
    tenant_id       UUID NOT NULL,
    scope           TEXT NOT NULL,
    tool            TEXT NOT NULL,
    args_redacted   JSONB NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'resolved', 'expired')),
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at      TIMESTAMPTZ,
    decided_by      TEXT
);

CREATE INDEX hotl_pending_tenant_id_idx ON hotl_pending (tenant_id);
CREATE INDEX hotl_pending_escalation_id_idx ON hotl_pending (escalation_id);
-- Boot replay query: status='pending' AND expires_at > now().
CREATE INDEX hotl_pending_status_expires_idx
    ON hotl_pending (status, expires_at)
    WHERE status = 'pending';

ALTER TABLE hotl_pending ENABLE ROW LEVEL SECURITY;

CREATE POLICY hotl_pending_tenant_isolation ON hotl_pending
    USING (tenant_id::text = current_setting('app.current_tenant_id', true));

-- 1-to-1 backfill (no-op on this branch — hotl_pending is a brand-new
-- table). The block stays here as a forward-compatible guard so deploys
-- that hand-rolled a flat hotl_pending schema between sprint-11 and
-- sprint-13 still land in a valid post-migration state. SET NOT NULL
-- then locks the FK once the backfill (or lack thereof) settles.
WITH orphans AS (
    SELECT id, tenant_id, scope, created_at
    FROM hotl_pending
    WHERE escalation_id IS NULL
), inserted AS (
    INSERT INTO hotl_escalations (id, tenant_id, session_id, top_level_scope, created_at, status)
    SELECT gen_random_uuid(), tenant_id, gen_random_uuid(), scope, created_at, 'pending'
    FROM orphans
    RETURNING id, tenant_id, top_level_scope, created_at
)
UPDATE hotl_pending p
SET escalation_id = i.id
FROM inserted i
WHERE p.escalation_id IS NULL
  AND p.tenant_id = i.tenant_id
  AND p.scope = i.top_level_scope
  AND p.created_at = i.created_at;

ALTER TABLE hotl_pending
    ALTER COLUMN escalation_id SET NOT NULL;

-- Per-tenant JSONPath redaction rules. `applies_to` is a small array so a
-- single policy can target both SSE emission and the audit row payload.
CREATE TABLE hotl_redaction_policies (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL,
    scope       TEXT NOT NULL,
    jsonpath    TEXT NOT NULL,
    applies_to  TEXT[] NOT NULL DEFAULT ARRAY['sse']::TEXT[],
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX hotl_redaction_policies_tenant_scope_idx
    ON hotl_redaction_policies (tenant_id, scope);

ALTER TABLE hotl_redaction_policies ENABLE ROW LEVEL SECURITY;

CREATE POLICY hotl_redaction_policies_tenant_isolation ON hotl_redaction_policies
    USING (tenant_id::text = current_setting('app.current_tenant_id', true));

-- Minimal Casbin-compatible seed table. Sprint-13's compiled-in
-- `rbac_policy.csv` does not reach this table yet — S13-10 wires the
-- DB-backed adapter so the `hotl:decide` scope rule below becomes
-- enforceable at runtime. Column shape follows the Casbin sql-adapter
-- convention (`ptype`, `v0`..`v5`) so future scope rules can land via
-- plain INSERTs without further DDL.
CREATE TABLE IF NOT EXISTS casbin_rule (
    id      BIGSERIAL PRIMARY KEY,
    ptype   VARCHAR(12) NOT NULL,
    v0      VARCHAR(128),
    v1      VARCHAR(128),
    v2      VARCHAR(128),
    v3      VARCHAR(128),
    v4      VARCHAR(128),
    v5      VARCHAR(128)
);

CREATE INDEX IF NOT EXISTS casbin_rule_ptype_idx ON casbin_rule (ptype);

-- Defensive removal of any path-based fallback rule a local deploy may
-- have hand-rolled before this migration. The compiled-in CSV never had
-- this rule (sprint-11/12 left /v1/hotl/decisions un-enforced); the
-- DELETE is a no-op on a fresh database.
DELETE FROM casbin_rule
WHERE ptype = 'p'
  AND v0 = '*'
  AND v1 = '/v1/hotl/decisions'
  AND v2 = 'POST';

INSERT INTO casbin_rule (ptype, v0, v1, v2, v3)
VALUES ('p', 'hotl:decide', '/v1/hotl/decisions', 'POST', 'allow');
