-- v1.1.2: conversation fork. A forked session points back at its parent and at
-- the message id we cut after. Both nullable (SQLite ADD COLUMN with a nullable
-- self-referential FK is permitted).
ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
ALTER TABLE sessions ADD COLUMN forked_from_message_id TEXT;
CREATE INDEX ix_sessions_parent ON sessions (parent_session_id) WHERE parent_session_id IS NOT NULL;
