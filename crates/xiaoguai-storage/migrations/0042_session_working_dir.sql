-- 0042: per-session coding workspace root (SQLite single-owner).
--
-- An absolute server-side path used as the coding tools' workspace root for
-- this session's turns (output / file-write base). NULL or empty = no
-- per-session override, fall back to the global default
-- (`XIAOGUAI_CODING_WORKSPACE`, resolved by `coding_workspace_root()`). Set via
-- `PATCH /v1/sessions/{id}`. The browser sends a plain path string.
ALTER TABLE sessions ADD COLUMN working_dir TEXT;
