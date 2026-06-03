-- v1.1.1.1: per-provider cost rates for the Usage pane (SQLite single-user).
--
-- Two nullable REAL columns: NULL means "no rate configured" and the UI surfaces
-- "—" rather than a misleading $0.00. SQLite adds one column per ALTER statement.
-- Seed rows drop the Postgres tenant_id (every provider is owner-wide).

ALTER TABLE llm_providers ADD COLUMN cost_per_1k_input_usd  REAL DEFAULT NULL;
ALTER TABLE llm_providers ADD COLUMN cost_per_1k_output_usd REAL DEFAULT NULL;

-- OpenAI
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('openai-gpt-4o', 'OpenAI GPT-4o', 'openai_compat',
     'https://api.openai.com/v1',
     '["gpt-4o"]', '["gpt-4o"]',
     10, 'OPENAI_API_KEY', 2.5, 10.0),
    ('openai-gpt-4o-mini', 'OpenAI GPT-4o Mini', 'openai_compat',
     'https://api.openai.com/v1',
     '["gpt-4o-mini"]', '["gpt-4o-mini"]',
     20, 'OPENAI_API_KEY', 0.15, 0.6)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- Anthropic
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('anthropic-claude-sonnet-4-6', 'Anthropic Claude Sonnet 4.6', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-sonnet-4-6"]', '["claude-sonnet-4-6"]',
     30, 'ANTHROPIC_API_KEY', 3.0, 15.0),
    ('anthropic-claude-haiku-4-5', 'Anthropic Claude Haiku 4.5', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-haiku-4-5"]', '["claude-haiku-4-5"]',
     31, 'ANTHROPIC_API_KEY', 1.0, 5.0),
    ('anthropic-claude-opus-4-7', 'Anthropic Claude Opus 4.7', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-opus-4-7"]', '["claude-opus-4-7"]',
     32, 'ANTHROPIC_API_KEY', 15.0, 75.0)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- DeepSeek
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('deepseek-chat', 'DeepSeek Chat', 'openai_compat',
     'https://api.deepseek.com/v1',
     '["deepseek-chat"]', '["deepseek-chat"]',
     40, 'DEEPSEEK_API_KEY', 0.27, 1.1)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- Qwen (Alibaba Cloud)
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('qwen-max', 'Qwen Max', 'openai_compat',
     'https://dashscope.aliyuncs.com/compatible-mode/v1',
     '["qwen-max"]', '["qwen-max"]',
     50, 'DASHSCOPE_API_KEY', 0.4, 1.2)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- Zhipu AI (GLM-4)
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('glm-4', 'Zhipu GLM-4', 'openai_compat',
     'https://open.bigmodel.cn/api/paas/v4',
     '["glm-4"]', '["glm-4"]',
     60, 'ZHIPU_API_KEY', 0.5, 1.5)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- Google Gemini
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('gemini-2.5-pro', 'Gemini 2.5 Pro', 'openai_compat',
     'https://generativelanguage.googleapis.com/v1beta/openai',
     '["gemini-2.5-pro"]', '["gemini-2.5-pro"]',
     70, 'GOOGLE_API_KEY', 1.25, 10.0),
    ('gemini-2.0-flash', 'Gemini 2.0 Flash', 'openai_compat',
     'https://generativelanguage.googleapis.com/v1beta/openai',
     '["gemini-2.0-flash"]', '["gemini-2.0-flash"]',
     71, 'GOOGLE_API_KEY', 0.1, 0.4)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;

-- Ollama (local, zero cost)
INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('ollama-local', 'Ollama (local)', 'ollama',
     'http://localhost:11434',
     '[]', '[]',
     90, NULL, 0.0, 0.0)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = excluded.cost_per_1k_input_usd,
        cost_per_1k_output_usd = excluded.cost_per_1k_output_usd;
