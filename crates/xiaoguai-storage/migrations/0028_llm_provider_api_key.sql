-- 0028: directly-stored API key for web-UI-registered LLM providers (SQLite single-user).
--
-- `api_key` (this column) wins over `api_key_env` at router build time; NULL
-- preserves the prior env-var behaviour. Acceptable for the single-user / personal
-- deployment this targets.

ALTER TABLE llm_providers ADD COLUMN api_key TEXT;
