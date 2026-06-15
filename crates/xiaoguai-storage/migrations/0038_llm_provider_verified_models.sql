-- 0038: per-provider connectivity-probe results (SQLite single-user).
--
-- JSON array of model ids that responded successfully to a live probe
-- (`POST /v1/admin/providers/{id}/probe`, which issues a minimal chat request
-- straight at this provider). NULL = never probed — the chat model picker then
-- falls back to advertising the full `models` list. A non-NULL value lists
-- exactly the models that connected, so the picker can offer only those.
ALTER TABLE llm_providers ADD COLUMN verified_models TEXT;
