-- migration 0018: task board — durable Kanban-style multi-agent work queue (ADR-0019, v1.4).
--
-- Three tables:
--   boards          — one board per tenant/team/pack, multi-tenant scoped.
--   tasks           — individual cards flowing through TRIAGE→DONE.
--   task_state_log  — append-only event history for every column transition
--                     (this IS the outcome-attribution chain).
--
-- Column enum: triage | todo | ready | running | blocked | done
-- (lowercase in DB; application layer maps to/from the display form.)

-- ---------------------------------------------------------------------------
-- boards
-- ---------------------------------------------------------------------------

CREATE TABLE boards (
    id              UUID        PRIMARY KEY,
    tenant_id       UUID        NOT NULL,
    name            TEXT        NOT NULL,
    default_board   BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Dispatch policy: 'fifo' | 'priority' | 'round_robin'
    dispatch_policy TEXT        NOT NULL DEFAULT 'fifo',
    -- Maximum number of simultaneously RUNNING cards on this board.
    pool_size       INT         NOT NULL DEFAULT 5,

    UNIQUE (tenant_id, name)
);

-- Only one default board per tenant.
CREATE UNIQUE INDEX boards_tenant_default_unique
    ON boards (tenant_id)
    WHERE default_board = TRUE;

CREATE INDEX boards_tenant_idx
    ON boards (tenant_id);

-- ---------------------------------------------------------------------------
-- tasks
-- ---------------------------------------------------------------------------

CREATE TABLE tasks (
    id              UUID        PRIMARY KEY,
    board_id        UUID        NOT NULL REFERENCES boards (id) ON DELETE CASCADE,

    -- Column (enum-as-text for schema simplicity; CHECK guards the value set).
    -- Named board_column, not column: "column" is a SQL reserved word, so an
    -- unquoted identifier of that name is a syntax error on every Postgres.
    board_column    VARCHAR(16) NOT NULL DEFAULT 'triage'
                    CHECK (board_column IN ('triage','todo','ready','running','blocked','done')),

    title           TEXT        NOT NULL,
    description     TEXT,

    -- 0–255; higher = more urgent. Used by the 'priority' dispatch policy.
    priority        INT         NOT NULL DEFAULT 128
                    CHECK (priority >= 0 AND priority <= 255),

    -- Optional agent identifier for affinity-based dispatch.
    assignee_agent  TEXT,

    -- Self-referential FK for sub-task grouping (deferred per ADR-0019 open q).
    parent_task_id  UUID        REFERENCES tasks (id) ON DELETE SET NULL,

    -- Populated on BLOCKED transition; cleared on re-dispatch.
    blocked_reason  TEXT,

    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Primary filter path: find all tasks on a board in a given column.
CREATE INDEX tasks_board_column_idx
    ON tasks (board_id, board_column);

-- Fetch all cards for a tenant across boards (used by the operator overview).
CREATE INDEX tasks_board_tenant_idx
    ON tasks (board_id);

-- Dispatch affinity filter: find unassigned READY cards for a specific agent tag.
CREATE INDEX tasks_assignee_idx
    ON tasks (assignee_agent)
    WHERE assignee_agent IS NOT NULL;

-- Priority-ordered READY cards for priority dispatch policy.
CREATE INDEX tasks_ready_priority_idx
    ON tasks (board_id, priority DESC, created_at ASC)
    WHERE board_column = 'ready';

-- ---------------------------------------------------------------------------
-- task_state_log  — append-only transition history
-- ---------------------------------------------------------------------------

CREATE TABLE task_state_log (
    id          BIGSERIAL   PRIMARY KEY,
    task_id     UUID        NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    from_column VARCHAR(16),   -- NULL on creation event
    to_column   VARCHAR(16)    NOT NULL,
    actor       TEXT        NOT NULL,  -- agent_id, user_id, or 'system'
    reason      TEXT,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Fast task history lookup (the attribution chain).
CREATE INDEX task_state_log_task_idx
    ON task_state_log (task_id, occurred_at);

-- Outcome telemetry join: find all transitions of a given type across a board.
CREATE INDEX task_state_log_to_column_idx
    ON task_state_log (to_column, occurred_at);

-- ---------------------------------------------------------------------------
-- Workspace linkage (v1.4 integration).
--
-- 0017_workspaces ran BEFORE this migration (tasks was renumbered 0016 -> 0018
-- to resolve the personas collision), so its conditional `boards`/`tasks`
-- workspace_id blocks no-op'd (the tables did not exist yet). We add the
-- linkage here, after the tables exist. Idempotent so re-ordering is safe.
-- ---------------------------------------------------------------------------
ALTER TABLE boards
    ADD COLUMN IF NOT EXISTS workspace_id UUID
    REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS boards_workspace_idx
    ON boards (workspace_id);

ALTER TABLE tasks
    ADD COLUMN IF NOT EXISTS workspace_id UUID
    REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS tasks_workspace_idx
    ON tasks (workspace_id);
