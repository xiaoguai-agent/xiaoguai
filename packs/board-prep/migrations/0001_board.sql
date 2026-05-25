-- Board Prep Pack — migration 0001
-- Creates core tables for board meeting lifecycle management:
--   board_meetings, agendas, minutes, action_items, board_members,
--   committee_assignments.
--
-- Tenant isolation: every row is scoped to tenant_id.
-- Privileged content: no PII from board_members is embedded in LLM
-- prompt columns; PII lives only in board_members and is joined at
-- query time by authorised server-side tools only.

-- ---------------------------------------------------------------------------
-- Board Members
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_members (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    full_name       TEXT        NOT NULL,
    title           TEXT,                          -- e.g. "Chair", "Lead Independent Director"
    email           TEXT        NOT NULL,
    -- PII: address + national ID held separately; never exposed to LLM prompts
    is_active       BOOLEAN     NOT NULL DEFAULT TRUE,
    joined_on       DATE,
    left_on         DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_members_tenant
    ON board_members (tenant_id)
    WHERE is_active = TRUE;

-- ---------------------------------------------------------------------------
-- Committee Assignments
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_committee_assignments (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    member_id       TEXT        NOT NULL REFERENCES board_members(id),
    committee_name  TEXT        NOT NULL,  -- e.g. "Audit", "Compensation", "Nominating"
    role            TEXT        NOT NULL DEFAULT 'member'
                                CHECK (role IN ('chair', 'member')),
    effective_from  DATE        NOT NULL,
    effective_to    DATE,                  -- NULL = current
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_committee_tenant_member
    ON board_committee_assignments (tenant_id, member_id);

CREATE INDEX IF NOT EXISTS board_committee_tenant_name
    ON board_committee_assignments (tenant_id, committee_name)
    WHERE effective_to IS NULL;

-- ---------------------------------------------------------------------------
-- Board Meetings
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_meetings (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    meeting_type    TEXT        NOT NULL DEFAULT 'regular'
                                CHECK (meeting_type IN ('regular', 'special', 'committee')),
    committee_name  TEXT,                  -- populated when meeting_type = 'committee'
    scheduled_at    TIMESTAMPTZ NOT NULL,
    location        TEXT,                  -- physical location or "virtual"
    zoom_meeting_id TEXT,                  -- populated when sourced from calendar
    status          TEXT        NOT NULL DEFAULT 'scheduled'
                                CHECK (status IN ('scheduled', 'in_progress', 'completed', 'cancelled')),
    chair_member_id TEXT        REFERENCES board_members(id),
    quorum_achieved BOOLEAN,               -- recorded during/after meeting
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_meetings_tenant_scheduled
    ON board_meetings (tenant_id, scheduled_at DESC);

CREATE INDEX IF NOT EXISTS board_meetings_tenant_status
    ON board_meetings (tenant_id, status)
    WHERE status IN ('scheduled', 'in_progress');

-- ---------------------------------------------------------------------------
-- Agendas
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_agendas (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    meeting_id      TEXT        NOT NULL REFERENCES board_meetings(id),
    version         INTEGER     NOT NULL DEFAULT 1,
    status          TEXT        NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft', 'approved', 'distributed')),
    approved_by     TEXT,                  -- member_id of corporate secretary/chair who approved
    approved_at     TIMESTAMPTZ,
    distributed_at  TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS board_agenda_items (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    agenda_id       TEXT        NOT NULL REFERENCES board_agendas(id),
    sequence        INTEGER     NOT NULL,   -- display order
    section         TEXT        NOT NULL    -- e.g. "standing", "prior-action-items", "new-business", "reports"
                                CHECK (section IN ('standing', 'prior-action-items', 'new-business', 'reports', 'closed-session')),
    title           TEXT        NOT NULL,
    description     TEXT,
    presenter_member_id TEXT    REFERENCES board_members(id),
    estimated_minutes INTEGER,
    -- if this item arose from an open action item, link it
    source_action_item_id TEXT, -- FK to board_action_items; deferring constraint for circular reference
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_agenda_items_agenda
    ON board_agenda_items (agenda_id, sequence);

-- ---------------------------------------------------------------------------
-- Minutes
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_minutes (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    meeting_id      TEXT        NOT NULL REFERENCES board_meetings(id) UNIQUE,
    version         INTEGER     NOT NULL DEFAULT 1,
    status          TEXT        NOT NULL DEFAULT 'draft'
                                CHECK (status IN ('draft', 'pending_approval', 'approved', 'archived')),
    draft_body      TEXT,                  -- full minutes markdown (LLM-generated)
    -- Formal resolution section (structured for legal record)
    resolutions     JSONB       NOT NULL DEFAULT '[]',
    approved_by     TEXT,                  -- corporate secretary or chair member_id
    approved_at     TIMESTAMPTZ,
    archived_at     TIMESTAMPTZ,
    worm_ref        TEXT,                  -- immutable archive reference after archival
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_minutes_tenant_meeting
    ON board_minutes (tenant_id, meeting_id);

CREATE INDEX IF NOT EXISTS board_minutes_status
    ON board_minutes (tenant_id, status)
    WHERE status IN ('draft', 'pending_approval');

-- ---------------------------------------------------------------------------
-- Action Items
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS board_action_items (
    id              TEXT        PRIMARY KEY DEFAULT gen_random_uuid()::TEXT,
    tenant_id       TEXT        NOT NULL,
    meeting_id      TEXT        NOT NULL REFERENCES board_meetings(id),
    -- originating agenda item (optional — some AIs arise mid-discussion)
    agenda_item_id  TEXT        REFERENCES board_agenda_items(id),
    description     TEXT        NOT NULL,
    owner_member_id TEXT        REFERENCES board_members(id),
    owner_text      TEXT,                  -- free-text owner when not a board member
    due_date        DATE        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'open'
                                CHECK (status IN ('open', 'in_progress', 'completed', 'overdue', 'deferred')),
    priority        TEXT        NOT NULL DEFAULT 'normal'
                                CHECK (priority IN ('urgent', 'normal', 'low')),
    -- escalation tracking
    reminder_sent_at    TIMESTAMPTZ,
    escalated_to_chair  BOOLEAN  NOT NULL DEFAULT FALSE,
    escalated_at        TIMESTAMPTZ,
    -- resolution
    completed_at    TIMESTAMPTZ,
    completion_note TEXT,
    -- carry-forward: if deferred, points to replacement item in next meeting
    carried_forward_to  TEXT,              -- action_item.id
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS board_action_items_tenant_open
    ON board_action_items (tenant_id, due_date)
    WHERE status IN ('open', 'in_progress', 'overdue');

CREATE INDEX IF NOT EXISTS board_action_items_owner
    ON board_action_items (tenant_id, owner_member_id)
    WHERE status IN ('open', 'in_progress', 'overdue');

CREATE INDEX IF NOT EXISTS board_action_items_meeting
    ON board_action_items (meeting_id);

-- Add the deferred FK from agenda_items to action_items
ALTER TABLE board_agenda_items
    ADD CONSTRAINT board_agenda_items_source_action_fk
    FOREIGN KEY (source_action_item_id) REFERENCES board_action_items(id)
    DEFERRABLE INITIALLY DEFERRED;

-- ---------------------------------------------------------------------------
-- updated_at auto-maintenance
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION board_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER board_members_updated_at
BEFORE UPDATE ON board_members
FOR EACH ROW EXECUTE FUNCTION board_set_updated_at();

CREATE OR REPLACE TRIGGER board_meetings_updated_at
BEFORE UPDATE ON board_meetings
FOR EACH ROW EXECUTE FUNCTION board_set_updated_at();

CREATE OR REPLACE TRIGGER board_agendas_updated_at
BEFORE UPDATE ON board_agendas
FOR EACH ROW EXECUTE FUNCTION board_set_updated_at();

CREATE OR REPLACE TRIGGER board_minutes_updated_at
BEFORE UPDATE ON board_minutes
FOR EACH ROW EXECUTE FUNCTION board_set_updated_at();

CREATE OR REPLACE TRIGGER board_action_items_updated_at
BEFORE UPDATE ON board_action_items
FOR EACH ROW EXECUTE FUNCTION board_set_updated_at();
