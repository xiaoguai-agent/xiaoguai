-- v1.11.x sprint-14 (S14-2, DEC-030/DEC-HLD-017): insert-only revisions for
-- `hotl_redaction_policies` + `tenant_settings.redaction_policy_required`
-- fail-closed flag.
--
-- Sprint-13's migration 0027 introduced `hotl_redaction_policies` as a flat
-- read-only table. Sprint-14 adds tenant-admin CRUD via insert-only revisions:
--
-- * PUT  /v1/admin/hotl-redaction-policies/{id}
--   → `supersede_policy`: tx that UPDATEs the prior row to active=false and
--     INSERTs a new row with `supersedes_policy_id = prior.id` AND `active = true`.
--     UPDATE-then-INSERT is **mandatory** under READ COMMITTED + the partial
--     unique index `WHERE active = true`: PostgreSQL enforces partial unique
--     constraints at statement boundary, and `CREATE UNIQUE INDEX ... WHERE`
--     cannot be DEFERRABLE (only `ALTER TABLE ... ADD CONSTRAINT ... DEFERRABLE`
--     can, and that path doesn't accept partial predicates). INSERT-first would
--     have both prior(active=true) and new(active=true) satisfy the predicate
--     simultaneously → 23505 unique_violation. UPDATE-first deactivates the
--     prior in the same tx; the new INSERT sees a clean partial-index slot.
--     Step-3 review caught this twice; the constraint is encoded by the
--     `supersede_policy` repo method tests in
--     `crates/xiaoguai-storage/tests/hotl_redaction_revisions.rs`.
--
-- * DELETE /v1/admin/hotl-redaction-policies/{id}
--   → `deactivate_policy`: single-statement UPDATE active=false. The row stays
--     in the table so audit FKs from prior `hotl_pending` events continue to
--     resolve (no rewrite of historical audit JSON; see DEC-HLD-017).
--
-- The `tenant_settings.redaction_policy_required` boolean implements
-- DEC-HLD-014's fail-closed mode: when true, `xiaoguai-auth::redaction::
-- RedactionRules::from_storage` should refuse to emit on the SSE path if no
-- active policy matches the scope. The default `false` preserves v1.10.x
-- "warn-once on empty rule set" behaviour.
--
-- Forward-only per GR-DB-02 — no DOWN migration.

-- ---------------------------------------------------------------------------
-- Revision columns on hotl_redaction_policies.
-- ---------------------------------------------------------------------------

ALTER TABLE hotl_redaction_policies
    ADD COLUMN supersedes_policy_id UUID
        REFERENCES hotl_redaction_policies (id) ON DELETE SET NULL;

ALTER TABLE hotl_redaction_policies
    ADD COLUMN active BOOLEAN NOT NULL DEFAULT TRUE;

-- `created_by`: backfill default `'system'` for the v1.10.x rows that
-- exist on upgrade paths; new INSERTs (after 0028 lands) always specify
-- the column explicitly via `insert_policy(..., created_by)`.
ALTER TABLE hotl_redaction_policies
    ADD COLUMN created_by TEXT NOT NULL DEFAULT 'system';

-- Partial unique index: one active rule per (tenant, scope, jsonpath).
-- This is the lock that makes `supersede_policy`'s UPDATE-then-INSERT
-- ordering load-bearing — see header comment.
--
-- Note: `CREATE UNIQUE INDEX ... WHERE` cannot be DEFERRABLE. Postgres
-- only allows DEFERRABLE on conventional UNIQUE/PK constraints added via
-- ALTER TABLE, and those don't support partial predicates. The repo
-- side compensates by sequencing inside a single transaction.
CREATE UNIQUE INDEX hotl_redaction_policies_active_uq
    ON hotl_redaction_policies (tenant_id, scope, jsonpath)
    WHERE active = TRUE;

-- Lookup index for the `get_revisions` chain walk. The view below uses
-- this implicitly via the recursive join on `supersedes_policy_id`.
CREATE INDEX hotl_redaction_policies_supersedes_idx
    ON hotl_redaction_policies (supersedes_policy_id)
    WHERE supersedes_policy_id IS NOT NULL;

-- Revision-chain view. Recursive CTE walks both forward (newer-than) and
-- backward (older-than) from a given policy id; callers `WHERE id = $1`
-- to anchor. Reverse-chronological ordering matches the
-- `get_revisions` contract.
--
-- The view does **not** filter by `active` — the whole point is to
-- expose superseded history. RLS on the underlying table still applies
-- when accessed under a tenant GUC.
CREATE VIEW hotl_redaction_policy_revisions AS
WITH RECURSIVE chain (anchor_id, id, tenant_id, scope, jsonpath, applies_to,
                     active, created_at, created_by, supersedes_policy_id) AS (
    -- Seed: every policy is the anchor of its own chain.
    SELECT
        p.id AS anchor_id,
        p.id, p.tenant_id, p.scope, p.jsonpath, p.applies_to,
        p.active, p.created_at, p.created_by, p.supersedes_policy_id
    FROM hotl_redaction_policies p

    UNION ALL

    -- Walk backward to older revisions via supersedes_policy_id.
    SELECT
        c.anchor_id,
        prev.id, prev.tenant_id, prev.scope, prev.jsonpath, prev.applies_to,
        prev.active, prev.created_at, prev.created_by, prev.supersedes_policy_id
    FROM chain c
    JOIN hotl_redaction_policies prev
        ON prev.id = c.supersedes_policy_id
)
SELECT * FROM chain;

-- ---------------------------------------------------------------------------
-- tenant_settings: fail-closed mode flag (DEC-HLD-014).
-- ---------------------------------------------------------------------------
--
-- Pre-existing `tenant_settings` (migration 0021) keys on TEXT tenant_id and
-- has a JSONB `settings` blob. We add a real column for `redaction_policy_required`
-- so the read path (S14-7 / SuspendingHotlGate) can `WHERE redaction_policy_required`
-- without JSONB key extraction at request time.

ALTER TABLE tenant_settings
    ADD COLUMN redaction_policy_required BOOLEAN NOT NULL DEFAULT FALSE;
