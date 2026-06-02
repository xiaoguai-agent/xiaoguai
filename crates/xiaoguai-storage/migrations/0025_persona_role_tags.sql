-- Sprint-9 S9-7 (DEC-021): persona role tags (SQLite single-user).
--
-- Postgres text[] -> TEXT holding a JSON array of tag strings. Convention:
-- role/{planner,worker,critic} plus domain/* prefixes. NULL or '[]' = untagged.
-- The pgvector-style GIN index and COMMENT ON COLUMN are dropped.

ALTER TABLE personas ADD COLUMN tags TEXT;
