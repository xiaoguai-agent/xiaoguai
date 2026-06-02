-- migration 0018: task board — durable Kanban-style work queue (SQLite single-user).
-- UUID -> TEXT; tenant_id dropped; VARCHAR(n) -> TEXT; CHECK constraints kept.
-- boards/tasks are created here, so workspace_id is added (unconditionally) after.

CREATE TABLE boards (
    id              TEXT        PRIMARY KEY,
    name            TEXT        NOT NULL,
    default_board   BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at      TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    -- Dispatch policy: 'fifo' | 'priority' | 'round_robin'
    dispatch_policy TEXT        NOT NULL DEFAULT 'fifo',
    pool_size       INTEGER     NOT NULL DEFAULT 5,
    UNIQUE (name)
);

-- Only one default board total (single owner).
CREATE UNIQUE INDEX boards_default_unique
    ON boards (default_board)
    WHERE default_board = TRUE;

CREATE TABLE tasks (
    id              TEXT        PRIMARY KEY,
    board_id        TEXT        NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    -- 'column' is a SQL reserved word; named board_column.
    board_column    TEXT        NOT NULL DEFAULT 'triage'
                    CHECK (board_column IN ('triage','todo','ready','running','blocked','done')),
    title           TEXT        NOT NULL,
    description     TEXT,
    priority        INTEGER     NOT NULL DEFAULT 128
                    CHECK (priority >= 0 AND priority <= 255),
    assignee_agent  TEXT,
    parent_task_id  TEXT        REFERENCES tasks (id) ON DELETE SET NULL,
    blocked_reason  TEXT,
    created_at      TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX tasks_board_column_idx ON tasks (board_id, board_column);
CREATE INDEX tasks_assignee_idx ON tasks (assignee_agent) WHERE assignee_agent IS NOT NULL;
CREATE INDEX tasks_ready_priority_idx
    ON tasks (board_id, priority DESC, created_at ASC)
    WHERE board_column = 'ready';

CREATE TABLE task_state_log (
    id          INTEGER     PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT        NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    from_column TEXT,
    to_column   TEXT        NOT NULL,
    actor       TEXT        NOT NULL,
    reason      TEXT,
    occurred_at TEXT        NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX task_state_log_task_idx ON task_state_log (task_id, occurred_at);
CREATE INDEX task_state_log_to_column_idx ON task_state_log (to_column, occurred_at);

-- Workspace linkage (0017 ran first but boards/tasks did not exist yet).
ALTER TABLE boards ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX boards_workspace_idx ON boards (workspace_id);
ALTER TABLE tasks ADD COLUMN workspace_id TEXT REFERENCES workspaces (id) ON DELETE SET NULL;
CREATE INDEX tasks_workspace_idx ON tasks (workspace_id);
