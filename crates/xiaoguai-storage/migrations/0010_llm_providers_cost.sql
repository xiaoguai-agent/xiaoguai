-- v1.1.1.1: per-provider cost rates for the Usage pane.
--
-- Adds two optional NUMERIC columns to `llm_providers` so the Usage
-- aggregation query can compute `cost_usd` without a separate catalog
-- lookup. Both are nullable: NULL means "no rate configured" and the UI
-- surfaces "—" rather than a misleading $0.00.
--
-- Prices sourced from 2026-Q2 public list pricing per provider docs.
-- Operators may UPDATE these rows to reflect negotiated / current rates.

-- FLOAT8 (= double precision) is used rather than NUMERIC so that sqlx
-- can decode the column into Rust f64 without the rust_decimal feature.
-- The precision loss vs. NUMERIC is negligible for pricing data (6 sig.
-- figs is well within double-precision range).
ALTER TABLE llm_providers
    ADD COLUMN cost_per_1k_input_usd  FLOAT8 DEFAULT NULL,
    ADD COLUMN cost_per_1k_output_usd FLOAT8 DEFAULT NULL;

-- Built-in / system provider catalog seed.
-- Rows are INSERT … ON CONFLICT DO NOTHING so re-running the migration
-- (or running against a DB that already has these ids) is idempotent.
-- All provider rows are system-wide (tenant_id IS NULL).
--
-- IDs use a short kebab-case slug convention matching the CLI register
-- defaults; operators who registered providers with different ids can
-- UPDATE those rows manually.

-- OpenAI
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('openai-gpt-4o', NULL, 'OpenAI GPT-4o', 'openai_compat',
     'https://api.openai.com/v1',
     '["gpt-4o"]'::jsonb, '["gpt-4o"]'::jsonb,
     10, 'OPENAI_API_KEY', 2.500000, 10.000000),

    ('openai-gpt-4o-mini', NULL, 'OpenAI GPT-4o Mini', 'openai_compat',
     'https://api.openai.com/v1',
     '["gpt-4o-mini"]'::jsonb, '["gpt-4o-mini"]'::jsonb,
     20, 'OPENAI_API_KEY', 0.150000, 0.600000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- Anthropic
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('anthropic-claude-sonnet-4-6', NULL, 'Anthropic Claude Sonnet 4.6', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-sonnet-4-6"]'::jsonb, '["claude-sonnet-4-6"]'::jsonb,
     30, 'ANTHROPIC_API_KEY', 3.000000, 15.000000),

    ('anthropic-claude-haiku-4-5', NULL, 'Anthropic Claude Haiku 4.5', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-haiku-4-5"]'::jsonb, '["claude-haiku-4-5"]'::jsonb,
     31, 'ANTHROPIC_API_KEY', 1.000000, 5.000000),

    ('anthropic-claude-opus-4-7', NULL, 'Anthropic Claude Opus 4.7', 'openai_compat',
     'https://api.anthropic.com/v1',
     '["claude-opus-4-7"]'::jsonb, '["claude-opus-4-7"]'::jsonb,
     32, 'ANTHROPIC_API_KEY', 15.000000, 75.000000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- DeepSeek
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('deepseek-chat', NULL, 'DeepSeek Chat', 'openai_compat',
     'https://api.deepseek.com/v1',
     '["deepseek-chat"]'::jsonb, '["deepseek-chat"]'::jsonb,
     40, 'DEEPSEEK_API_KEY', 0.270000, 1.100000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- Qwen (Alibaba Cloud)
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('qwen-max', NULL, 'Qwen Max', 'openai_compat',
     'https://dashscope.aliyuncs.com/compatible-mode/v1',
     '["qwen-max"]'::jsonb, '["qwen-max"]'::jsonb,
     50, 'DASHSCOPE_API_KEY', 0.400000, 1.200000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- Zhipu AI (GLM-4)
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('glm-4', NULL, 'Zhipu GLM-4', 'openai_compat',
     'https://open.bigmodel.cn/api/paas/v4',
     '["glm-4"]'::jsonb, '["glm-4"]'::jsonb,
     60, 'ZHIPU_API_KEY', 0.500000, 1.500000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- Google Gemini
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('gemini-2.5-pro', NULL, 'Gemini 2.5 Pro', 'openai_compat',
     'https://generativelanguage.googleapis.com/v1beta/openai',
     '["gemini-2.5-pro"]'::jsonb, '["gemini-2.5-pro"]'::jsonb,
     70, 'GOOGLE_API_KEY', 1.250000, 10.000000),

    ('gemini-2.0-flash', NULL, 'Gemini 2.0 Flash', 'openai_compat',
     'https://generativelanguage.googleapis.com/v1beta/openai',
     '["gemini-2.0-flash"]'::jsonb, '["gemini-2.0-flash"]'::jsonb,
     71, 'GOOGLE_API_KEY', 0.100000, 0.400000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;

-- Ollama (local, zero cost) — id pattern 'ollama-*'.
-- We insert a representative row; actual Ollama deployments may have any
-- model name. The operator registers concrete Ollama providers via the
-- CLI; this row is just a canonical example with $0 rates.
INSERT INTO llm_providers
    (id, tenant_id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('ollama-local', NULL, 'Ollama (local)', 'ollama',
     'http://localhost:11434',
     '[]'::jsonb, '[]'::jsonb,
     90, NULL, 0.000000, 0.000000)
ON CONFLICT (id) DO UPDATE
    SET cost_per_1k_input_usd  = EXCLUDED.cost_per_1k_input_usd,
        cost_per_1k_output_usd = EXCLUDED.cost_per_1k_output_usd;
