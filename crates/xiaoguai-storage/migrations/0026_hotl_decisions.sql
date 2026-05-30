-- v1.8.x sprint-11 (S11-3a.1): HOTL decision-record ledger.
--
-- Sibling table to `hotl_policies` (migration 0011). One row per
-- human decision recorded against an escalated HOTL request. The
-- agent loop in 3a.1 does NOT yet suspend on `Escalate`; this table
-- is therefore a record-of-decision only — the `resumed` field on
-- `HotlDecisionResponse` is always `false`. Full suspend/resume
-- (`SuspendingHotlGate`, `AgentEvent::HotlPending`, `DecisionRegistry`)
-- ships in a future sprint and will populate / look up rows here.
--
-- Columns:
--   `request_id`       — UNIQUE; equals the SSE `escalation_id` the
--                        chat-ui banner surfaces. Duplicate POSTs
--                        with the same id return 409.
--   `verdict`          — `allow` | `deny`. CHECK constraint enforced.
--   `decided_by`       — caller-supplied actor (e.g. user id, email).
--                        Once auth-identity lands the route will
--                        prefer Claims; the column stays.
--   `raised_policy_id` — soft FK to `hotl_policies.id`. NOT a real
--                        FK because some deploys may run this table
--                        without the policy table (split-deployments,
--                        replay environments). Nullable.

CREATE TABLE hotl_decisions (
    id               UUID PRIMARY KEY,
    request_id       UUID NOT NULL UNIQUE,
    tenant_id        UUID NOT NULL,
    verdict          TEXT NOT NULL CHECK (verdict IN ('allow', 'deny')),
    decided_by       TEXT NOT NULL,
    raised_policy_id UUID,
    recorded_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX hotl_decisions_tenant_recent
    ON hotl_decisions (tenant_id, recorded_at DESC);
