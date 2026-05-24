-- v1.1.2: conversation fork.
-- A forked session points back at its parent (so we can show lineage)
-- and at the message ID we cut after (so the UI can render the
-- "branched from this turn" breadcrumb later). Both nullable —
-- the vast majority of rows have no parent.
ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
ALTER TABLE sessions ADD COLUMN forked_from_message_id TEXT;
CREATE INDEX ix_sessions_parent ON sessions (parent_session_id) WHERE parent_session_id IS NOT NULL;
