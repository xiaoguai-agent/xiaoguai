//! Builders that translate persisted `LlmProvider` rows into a wired
//! [`LlmRouter`].
//!
//! Stateless helpers, pure over the row list + an env-var resolver. The
//! `xiaoguai-core` binary calls these on boot after pulling rows from
//! the `SQLite` store; unit tests can drive them with fixture data and an
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
/// Resolve a provider's API key: a directly-stored `api_key` (web-UI
/// providers) wins over the `api_key_env` env-var indirection. Returns `None`
/// when neither yields a value (the caller decides whether that's a warning).
fn resolve_optional_key(
    row: &xiaoguai_types::LlmProvider,
    env: &dyn EnvResolver,
) -> Option<String> {
    if let Some(k) = row.api_key.as_deref() {
        if !k.is_empty() {
            return Some(k.to_string());
        }
    }
    row.api_key_env.as_deref().and_then(|key| env.get(key))
}

fn resolve_required_key(
    row: &xiaoguai_types::LlmProvider,
    env: &dyn EnvResolver,
    report: &mut BuildReport,
) -> String {
    if let Some(v) = resolve_optional_key(row, env) {
        return v;
    }
    if let Some(key) = row.api_key_env.as_deref() {
        report.warn(format!(
            "provider {} ({}): env var {key} is unset and no stored api_key; \
             backend will be unauthenticated",
            row.name,
            row.id.as_str()
        ));
    } else {
        report.warn(format!(
            "provider {} ({}): no api_key or api_key_env set for {} provider",
            row.name,
            row.id.as_str(),
            row.kind.as_str()
        ));
    }
    String::new()
}

/// A provider that *requires* an API key (by its `ProviderKind`) but has none
/// configured (neither a stored `api_key` nor a resolvable `api_key_env`).
///
/// Such a provider builds an unauthenticated backend that can only ever return
/// HTTP 401, so it must be kept out of automatic routing — see [`build_router`]
/// and bug #17. Local / no-auth kinds (`Ollama`, `OpenAiCompat`) return `false`
/// even without a key, so legitimate self-hosted providers stay routable.
fn is_keyless_required(row: &LlmProvider, env: &dyn EnvResolver) -> bool {
    row.kind.requires_api_key() && resolve_optional_key(row, env).is_none()
}

/// Build an [`LlmRouter`] from a slice of `LlmProvider` rows.
///
/// Order of operations:
///   1. Construct one backend per row according to its `ProviderKind`.
///   2. Build a [`RouterConfig`]:
///      - `system_default_for_model` from each row's `default_for_models`.
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
                let api_key = resolve_optional_key(row, env);
                if api_key.is_none() && row.api_key_env.is_some() {
                    report.warn(format!(
                        "provider {} ({}): env var {} is unset and no stored api_key; \
                         backend will run unauthenticated",
                        row.name,
                        row.id.as_str(),
                        row.api_key_env.as_deref().unwrap_or("")
                    ));
                }
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

    // Sort rows for deterministic fallback order.
    let mut globals: Vec<&LlmProvider> = rows.iter().collect();
    globals.sort_by(|a, b| {
        a.fallback_order
            .cmp(&b.fallback_order)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    let mut config = RouterConfig::default();
    for row in &globals {
        // Bug #17: a provider whose kind *requires* a key but has none
        // configured can only ever return HTTP 401. Its backend is still built
        // and kept in `backends` (so an explicit-provider route and the
        // single-provider connectivity probe still reach it), but it must not
        // become an automatic routing candidate — neither the default model nor
        // a fallback link — or it poisons every keyless turn. Skip it from the
        // candidacy maps with a warning.
        if is_keyless_required(row, env) {
            report.warn(format!(
                "provider {} ({}): {} requires an API key but none is configured; \
                 excluded from default/fallback routing (configure a key to enable it)",
                row.name,
                row.id.as_str(),
                row.kind.as_str()
            ));
            continue;
        }
        config.fallback_order.push(row.id.clone());
        for model in &row.default_for_models {
            // First-writer wins so the lowest fallback_order takes precedence.
            config
                .system_default_for_model
                .entry(model.clone())
                .or_insert_with(|| row.id.clone());
        }
    }

    // Default model for requests that omit one: the first model of the primary
    // (lowest fallback_order) provider that actually built a backend AND is a
    // valid routing candidate (not a keyless key-required provider — see #17).
    // A single-provider deployment then needs no `--model`; promoting a provider
    // (lower fallback_order) makes its model the default.
    config.default_model = globals
        .iter()
        .find(|row| {
            backends.contains_key(&row.id)
                && !row.models.is_empty()
                && !is_keyless_required(row, env)
        })
        .and_then(|row| row.models.first().cloned());

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
            name: name.to_string(),
            kind,
            endpoint: endpoint.to_string(),
            models: defaults.iter().map(|m| (*m).to_string()).collect(),
            default_for_models: defaults.iter().map(|m| (*m).to_string()).collect(),
            verified_models: None,
            fallback_order: order,
            api_key_env: api_key_env.map(str::to_string),
            api_key: None,
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
            session_id: None,
            user_id: None,
            request_id: None,
        };
        assert!(router.resolve(ctx, &req).is_empty());
    }

    #[test]
    fn default_model_is_primary_providers_first_model() {
        let env = no_env;
        let primary = provider(
            "p1",
            ProviderKind::Ollama,
            "http://a",
            vec!["model-a"],
            1,
            None,
        );
        let secondary = provider(
            "p2",
            ProviderKind::Ollama,
            "http://b",
            vec!["model-b"],
            50,
            None,
        );
        // Pass out of order to prove it's fallback_order, not arg order.
        let (router, _) = build_router(&[secondary, primary], &env);
        assert_eq!(router.default_model(), Some("model-a"));
    }

    #[test]
    fn default_model_is_none_when_provider_has_no_models() {
        let env = no_env;
        // A provider that lists no models can't supply a default.
        let p = provider("p", ProviderKind::Ollama, "http://a", vec![], 1, None);
        let (router, _) = build_router(&[p], &env);
        assert_eq!(router.default_model(), None);
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
            session_id: None,
            user_id: None,
            request_id: None,
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
            session_id: None,
            user_id: None,
            request_id: None,
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

    #[test]
    fn stored_api_key_wins_over_env_var() {
        // Web-UI providers carry the key directly; it must win even when the
        // env var is unset, and produce no "unauthenticated" warning.
        let mut row = provider(
            "minimax-web",
            ProviderKind::MiniMax,
            "https://api.minimax.io",
            vec!["MiniMax-M2"],
            100,
            Some("MISSING_ENV"),
        );
        row.api_key = Some("stored-secret".to_string());
        let empty = |_: &str| None;
        assert_eq!(
            resolve_optional_key(&row, &empty),
            Some("stored-secret".to_string())
        );
        let (_, report) = build_router(&[row], &empty);
        assert!(report.warnings.is_empty(), "stored key → no warning");
    }

    #[test]
    fn falls_back_to_env_when_no_stored_key() {
        let row = provider(
            "deepseek",
            ProviderKind::OpenAiCompat,
            "https://api.deepseek.com/v1",
            vec![],
            100,
            Some("DEEPSEEK_API_KEY"),
        );
        let resolver = |k: &str| (k == "DEEPSEEK_API_KEY").then(|| "from-env".to_string());
        assert_eq!(
            resolve_optional_key(&row, &resolver),
            Some("from-env".to_string())
        );
    }

    /// Build a minimal `ChatRequest` for `model` so tests can drive `resolve`.
    fn req_for(model: &str) -> crate::types::ChatRequest {
        crate::types::ChatRequest {
            model: model.into(),
            messages: vec![],
            tools: vec![],
            tool_choice: crate::ToolChoice::Auto,
            temperature: None,
            max_tokens: None,
            session_id: None,
            user_id: None,
            request_id: None,
        }
    }

    // ---- Bug #17: keyless key-required providers must not poison routing ----

    #[test]
    fn keyless_key_required_provider_is_not_a_routing_candidate() {
        // Mirrors the seeded `minimax-system` row: MiniMax kind, an api_key_env
        // pointer but no key resolvable, lowest fallback_order so it WOULD be
        // the default/primary candidate under the old logic.
        let env = no_env;
        let keyless = provider(
            "minimax",
            ProviderKind::MiniMax,
            "https://api.minimaxi.com",
            vec!["MiniMax-M2"],
            1,
            Some("MINIMAX_API_KEY"),
        );
        let keyless_id = keyless.id.clone();
        let (router, report) = build_router(&[keyless], &env);

        // (a) excluded from the default model entirely…
        assert_eq!(
            router.default_model(),
            None,
            "a keyless MiniMax provider must not become the default model (would 401)"
        );
        // …and from the fallback/default-for-model resolution.
        let resolved = router.resolve(crate::ResolveCtx::default(), &req_for("MiniMax-M2"));
        assert!(
            !resolved.contains(&keyless_id),
            "keyless key-required provider must not be a routing candidate, got {resolved:?}"
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("excluded from")),
            "build should warn that the keyless provider was excluded; warnings={:?}",
            report.warnings
        );
    }

    #[test]
    fn keyless_local_ollama_provider_is_still_a_candidate() {
        // Correctness guard: Ollama is keyless BY DESIGN (local, no auth). It
        // must stay a valid default + fallback candidate even with no key.
        let env = no_env;
        let ollama = provider(
            "local",
            ProviderKind::Ollama,
            "http://localhost:11434",
            vec!["llama3"],
            10,
            None,
        );
        let ollama_id = ollama.id.clone();
        let (router, _) = build_router(&[ollama], &env);

        assert_eq!(
            router.default_model(),
            Some("llama3"),
            "a keyless local Ollama provider must remain the deployment default"
        );
        let resolved = router.resolve(crate::ResolveCtx::default(), &req_for("llama3"));
        assert_eq!(
            resolved,
            vec![ollama_id],
            "Ollama must still route despite having no API key"
        );
    }

    #[test]
    fn keyed_key_required_provider_routes_as_before() {
        // A MiniMax provider WITH a stored key behaves exactly as before the
        // fix: it is the default and a fallback candidate.
        let env = no_env;
        let mut keyed = provider(
            "minimax-web",
            ProviderKind::MiniMax,
            "https://api.minimaxi.com",
            vec!["MiniMax-M2"],
            1,
            Some("MINIMAX_API_KEY"),
        );
        keyed.api_key = Some("sk-cp-real".to_string());
        let keyed_id = keyed.id.clone();
        let (router, report) = build_router(&[keyed], &env);

        assert_eq!(router.default_model(), Some("MiniMax-M2"));
        let resolved = router.resolve(crate::ResolveCtx::default(), &req_for("MiniMax-M2"));
        assert_eq!(resolved, vec![keyed_id]);
        assert!(
            report.warnings.is_empty(),
            "a keyed provider should build cleanly with no warnings; got {:?}",
            report.warnings
        );
    }

    #[test]
    fn keyless_required_provider_excluded_but_keyed_peer_still_routes() {
        // Demo scenario: the keyless seed (lower fallback_order) must NOT win
        // the default over a real keyed provider sitting behind it.
        let env = no_env;
        let keyless_primary = provider(
            "minimax-system",
            ProviderKind::MiniMax,
            "https://api.minimaxi.com",
            vec!["MiniMax-M1"],
            1, // lowest order → would be primary/default under old logic
            Some("MINIMAX_API_KEY"),
        );
        let keyless_id = keyless_primary.id.clone();
        let mut keyed_secondary = provider(
            "openai-real",
            ProviderKind::OpenAiCompat,
            "https://api.openai.com/v1",
            vec!["gpt-4o-mini"],
            100,
            Some("OPENAI_API_KEY"),
        );
        keyed_secondary.api_key = Some("sk-real".to_string());
        let keyed_id = keyed_secondary.id.clone();

        let (router, _) = build_router(&[keyless_primary, keyed_secondary], &env);

        // The keyed (higher-order) provider becomes the default, not the
        // keyless primary that would otherwise 401.
        assert_eq!(
            router.default_model(),
            Some("gpt-4o-mini"),
            "default must skip the keyless primary and land on the keyed provider"
        );
        let resolved = router.resolve(crate::ResolveCtx::default(), &req_for("MiniMax-M1"));
        assert!(
            !resolved.contains(&keyless_id),
            "keyless primary must be absent from the fallback chain"
        );
        assert_eq!(
            resolved,
            vec![keyed_id],
            "only the keyed provider should be a candidate"
        );
    }
}
