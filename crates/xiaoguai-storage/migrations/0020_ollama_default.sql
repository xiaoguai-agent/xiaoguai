-- migration 0020: make local Ollama the default LLM backend (SQLite single-user).
-- Promotes the seeded `ollama-local` row to fallback_order 1 with a default model.
-- Idempotent via ON CONFLICT.

INSERT INTO llm_providers
    (id, name, kind, endpoint, models, default_for_models,
     fallback_order, api_key_env, cost_per_1k_input_usd, cost_per_1k_output_usd)
VALUES
    ('ollama-local', 'Ollama (local)', 'ollama',
     'http://localhost:11434',
     '["qwen2.5-coder","llama3.2","mistral"]',
     '["qwen2.5-coder"]',
     1, NULL, 0.0, 0.0)
ON CONFLICT (id) DO UPDATE
    SET endpoint           = excluded.endpoint,
        models             = excluded.models,
        default_for_models = excluded.default_for_models,
        fallback_order     = excluded.fallback_order;
