//! Builders that translate persisted `LlmProvider` rows into a wired
//! [`LlmRouter`].
//!
//! Stateless helpers, pure over the row list + an env-var resolver. The
//! `xiaoguai-core` binary calls these on boot after pulling rows from
//! Postgres; unit tests can drive them with fixture data and an
//! in-memory resolver.

use std::collections::HashMap;
use std::sync::Arc;

use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

use crate::anthropic::AnthropicBackend;
use crate::azure_openai::AzureOpenAiBackend;
use crate::backend::LlmBackend;
use crate::bedrock::BedrockBackend;
use crate::gemini::GeminiBackend;
use crate::groq::GroqBackend;
use crate::minimax::{MinimaxBackend, MINIMAX_DEFAULT_BASE};
use crate::mistral::MistralBackend;
use crate::ollama::OllamaBackend;
use crate::openai_compat::OpenAiCompatBackend;
use crate::router::{LlmRouter, RouterConfig};

/// Warnings collected while building the router. Returned alongside the
/// router so the binary can log them; nothing here is fatal — a row with
/// a missing API-key env var simply produces an unauthenticated backend
/// (which most upstreams will reject at call time, but boot still
/// succeeds so that operators can fix the env without restarting all
/// providers).
#[derive(Debug, Default)]
pub struct BuildReport {
    pub warnings: Vec<String>,
}

impl BuildReport {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// Resolve an env-var name into its value. Production callers pass
/// [`std::env::var`]; tests pass a closure backed by a `HashMap`.
pub trait EnvResolver: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
}

impl<F> EnvResolver for F
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn get(&self, key: &str) -> Option<String> {
        self(key)
    }
}

/// Real-world resolver delegating to `std::env::var`.
pub struct OsEnvResolver;

impl EnvResolver for OsEnvResolver {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Resolve the API key env-var for a provider that *requires* a key.
/// Returns the key value or an empty string (with a warning) if unset.
fn resolve_required_key(
    row: &xiaoguai_types::LlmProvider,
    env: &dyn EnvResolver,
    report: &mut BuildReport,
) -> String {
    if let Some(key) = row.api_key_env.as_deref() {
        if let Some(v) = env.get(key) {
            return v;
        }
        report.warn(format!(
            "provider {} ({}): env var {key} is unset; backend will be unauthenticated",
            row.name,
            row.id.as_str()
        ));
    } else {
        report.warn(format!(
            "provider {} ({}): no api_key_env set for {} provider",
            row.name,
            row.id.as_str(),
            row.kind.as_str()
        ));
    }
    String::new()
}

/// Build an [`LlmRouter`] from a slice of `LlmProvider` rows.
///
/// Order of operations:
///   1. Construct one backend per row according to its `ProviderKind`.
///   2. Build a [`RouterConfig`]:
///      - `system_default_for_model` from each row's `default_for_models`
///        (system-wide rows only — tenant-scoped defaults are a v0.6.2
///        deferred item).
///      - `fallback_order` from system-wide rows sorted by their stored
///        `fallback_order` field, breaking ties by `created_at` ascending.
///
/// Returns the router plus a [`BuildReport`] enumerating any warnings
/// surfaced during construction.
#[must_use]
pub fn build_router(rows: &[LlmProvider], env: &dyn EnvResolver) -> (LlmRouter, BuildReport) {
    let mut report = BuildReport::default();
    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::with_capacity(rows.len());

    for row in rows {
        let backend: Arc<dyn LlmBackend> = match row.kind {
            ProviderKind::Ollama => Arc::new(OllamaBackend::new(row.endpoint.clone())),
            ProviderKind::OpenAiCompat => {
                let api_key = row.api_key_env.as_deref().and_then(|key| {
                    let v = env.get(key);
                    if v.is_none() {
                        report.warn(format!(
                            "provider {} ({}): env var {key} is unset; backend will run \
                             unauthenticated",
                            row.name,
                            row.id.as_str()
                        ));
                    }
                    v
                });
                Arc::new(OpenAiCompatBackend::new(row.endpoint.clone(), api_key))
            }
            ProviderKind::Anthropic => {
                let api_key = resolve_required_key(row, env, &mut report);
                Arc::new(AnthropicBackend::new(row.endpoint.clone(), api_key))
            }
            ProviderKind::Gemini => {
                let api_key = resolve_required_key(row, env, &mut report);
                Arc::new(GeminiBackend::with_base_url(row.endpoint.clone(), api_key))
            }
            ProviderKind::Bedrock => {
                // For Bedrock the `endpoint` field stores the AWS region.
                // Credentials come from env vars resolved at build time.
                let region = if row.endpoint.is_empty() {
                    "us-east-1".to_string()
                } else {
                    row.endpoint.clone()
                };
                let access_key = env.get("AWS_ACCESS_KEY_ID").unwrap_or_else(|| {
                    report.warn(format!(
                        "provider {} ({}): AWS_ACCESS_KEY_ID unset; Bedrock calls will fail",
                        row.name,
                        row.id.as_str()
                    ));
                    String::new()
                });
                let secret_key = env.get("AWS_SECRET_ACCESS_KEY").unwrap_or_else(|| {
                    report.warn(format!(
                        "provider {} ({}): AWS_SECRET_ACCESS_KEY unset; Bedrock calls will fail",
                        row.name,
                        row.id.as_str()
                    ));
                    String::new()
                });
                let session_token = env.get("AWS_SESSION_TOKEN");
                Arc::new(BedrockBackend::with_config(
                    region,
                    access_key,
                    secret_key,
                    session_token,
                    None,
                ))
            }
            ProviderKind::AzureOpenAi => {
                // For Azure the `endpoint` stores the full deployment URL:
                // `https://{resource}.openai.azure.com/openai/deployments/{deployment}`
                let api_key = resolve_required_key(row, env, &mut report);
                Arc::new(AzureOpenAiBackend::with_endpoint(
                    row.endpoint.clone(),
                    api_key,
                ))
            }
            ProviderKind::Mistral => {
                let api_key = resolve_required_key(row, env, &mut report);
                // `endpoint` may be empty → use default Mistral base URL.
                let base_url = if row.endpoint.is_empty() {
                    crate::mistral::MISTRAL_DEFAULT_BASE.to_string()
                } else {
                    row.endpoint.clone()
                };
                Arc::new(MistralBackend::with_base_url(base_url, api_key))
            }
            ProviderKind::Groq => {
                let api_key = resolve_required_key(row, env, &mut report);
                // `endpoint` may be empty → use default Groq base URL.
                let base_url = if row.endpoint.is_empty() {
                    crate::groq::GROQ_DEFAULT_BASE.to_string()
                } else {
                    row.endpoint.clone()
                };
                Arc::new(GroqBackend::with_base_url(base_url, api_key))
            }
            ProviderKind::MiniMax => {
                let api_key = resolve_required_key(row, env, &mut report);
                // `endpoint` may be empty → use default MiniMax base URL.
                let base_url = if row.endpoint.is_empty() {
                    MINIMAX_DEFAULT_BASE.to_string()
                } else {
                    row.endpoint.clone()
                };
                Arc::new(MinimaxBackend::with_base_url(base_url, api_key))
            }
        };
        if backends.insert(row.id.clone(), backend).is_some() {
            report.warn(format!(
                "duplicate provider id {} — keeping the last one",
                row.id.as_str()
            ));
        }
    }

    // Sort system-wide rows for deterministic fallback order. Tenant rows
    // would feed into `tenant_default_for_model`; we only handle globals
    // in this slice.
    let mut globals: Vec<&LlmProvider> = rows.iter().filter(|r| r.tenant_id.is_none()).collect();
    globals.sort_by(|a, b| {
        a.fallback_order
            .cmp(&b.fallback_order)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    let mut config = RouterConfig::default();
    for row in &globals {
        config.fallback_order.push(row.id.clone());
        for model in &row.default_for_models {
            // First-writer wins so the lowest fallback_order takes precedence.
            config
                .system_default_for_model
                .entry(model.clone())
                .or_insert_with(|| row.id.clone());
        }
    }

    (LlmRouter::new(backends, config), report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use xiaoguai_types::ids::ProviderId;

    fn provider(
        name: &str,
        kind: ProviderKind,
        endpoint: &str,
        defaults: Vec<&str>,
        order: i32,
        api_key_env: Option<&str>,
    ) -> LlmProvider {
        let now = Utc::now();
        LlmProvider {
            id: ProviderId::new(),
            tenant_id: None,
            name: name.to_string(),
            kind,
            endpoint: endpoint.to_string(),
            models: defaults.iter().map(|m| (*m).to_string()).collect(),
            default_for_models: defaults.iter().map(|m| (*m).to_string()).collect(),
            fallback_order: order,
            api_key_env: api_key_env.map(str::to_string),
            created_at: now,
            updated_at: now,
            cost_per_1k_input_usd: None,
            cost_per_1k_output_usd: None,
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn empty_input_yields_empty_router() {
        let env = no_env;
        let (router, report) = build_router(&[], &env);
        assert!(report.warnings.is_empty());
        // No backends, no defaults — resolve must return an empty candidate list.
        let ctx = crate::ResolveCtx::default();
        let req = crate::types::ChatRequest {
            model: "anything".into(),
            messages: vec![],
            tools: vec![],
            tool_choice: crate::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            tenant_id: None,
        };
        assert!(router.resolve(ctx, &req).is_empty());
    }

    #[test]
    fn fallback_order_orders_by_priority_then_created_at() {
        let env = no_env;
        let a = provider("a", ProviderKind::Ollama, "http://a", vec![], 100, None);
        let b = provider("b", ProviderKind::Ollama, "http://b", vec![], 50, None);
        let c = provider("c", ProviderKind::Ollama, "http://c", vec![], 50, None);
        // Force c.created_at later than b.created_at so the tie-break is
        // observable.
        let mut c = c;
        c.created_at = b.created_at + chrono::Duration::seconds(1);
        let (router, _) = build_router(&[a.clone(), c.clone(), b.clone()], &env);
        let ctx = crate::ResolveCtx::default();
        let req = crate::types::ChatRequest {
            model: "x".into(),
            messages: vec![],
            tools: vec![],
            tool_choice: crate::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            tenant_id: None,
        };
        let resolved = router.resolve(ctx, &req);
        assert_eq!(resolved, vec![b.id.clone(), c.id.clone(), a.id.clone()]);
    }

    #[test]
    fn system_default_for_model_uses_lowest_priority_row() {
        let env = no_env;
        let a = provider(
            "low",
            ProviderKind::Ollama,
            "http://a",
            vec!["m1"],
            200,
            None,
        );
        let b = provider(
            "high",
            ProviderKind::Ollama,
            "http://b",
            vec!["m1"],
            10,
            None,
        );
        let (router, _) = build_router(&[a, b.clone()], &env);
        let ctx = crate::ResolveCtx::default();
        let req = crate::types::ChatRequest {
            model: "m1".into(),
            messages: vec![],
            tools: vec![],
            tool_choice: crate::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            tenant_id: None,
        };
        let resolved = router.resolve(ctx, &req);
        assert_eq!(
            resolved.first().expect("resolved"),
            &b.id,
            "lowest fallback_order should win the model default"
        );
    }

    #[test]
    fn missing_api_key_env_emits_warning_but_proceeds() {
        let env = no_env;
        let row = provider(
            "deepseek",
            ProviderKind::OpenAiCompat,
            "https://api.deepseek.com/v1",
            vec![],
            100,
            Some("DEEPSEEK_API_KEY"),
        );
        let (_, report) = build_router(&[row], &env);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("DEEPSEEK_API_KEY"));
    }

    #[test]
    fn ollama_kind_ignores_api_key_env() {
        let env = no_env;
        let row = provider(
            "local",
            ProviderKind::Ollama,
            "http://localhost:11434",
            vec![],
            100,
            Some("DOES_NOT_MATTER"),
        );
        let (_, report) = build_router(&[row], &env);
        assert!(
            report.warnings.is_empty(),
            "Ollama backend does not consume api_key_env"
        );
    }

    #[test]
    fn map_resolver_finds_present_keys() {
        let map: std::collections::HashMap<String, String> = [("DEEPSEEK_API_KEY", "sk-test")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let resolver = move |k: &str| map.get(k).cloned();
        let row = provider(
            "deepseek",
            ProviderKind::OpenAiCompat,
            "https://api.deepseek.com/v1",
            vec![],
            100,
            Some("DEEPSEEK_API_KEY"),
        );
        let (_, report) = build_router(&[row], &resolver);
        assert!(report.warnings.is_empty());
    }
}
