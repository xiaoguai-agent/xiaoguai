-- 0028: directly-stored API key for web-UI–registered LLM providers.
--
-- Until now an LLM provider's credential was an env-var NAME (`api_key_env`),
-- which only works for operators who can set process env vars. The admin-ui
-- Providers pane lets a user paste a key for a hosted API (MiniMax, Zhipu,
-- OpenAI/codex, …) or leave it blank for a local URL (Ollama / OpenAI-compatible
-- server). Such keys are stored here.
--
-- Precedence at router build time (xiaoguai-llm::build): `api_key` (this column)
-- wins over `api_key_env`. NULL preserves the prior env-var behaviour, so every
-- seeded provider keeps working unchanged.
--
-- NOTE: this stores the secret in the application database. That is acceptable
-- for the single-tenant / personal deployments this targets; multi-tenant
-- operators should prefer `api_key_env` + a secrets manager.

ALTER TABLE llm_providers ADD COLUMN IF NOT EXISTS api_key TEXT;
