//! Live connectivity probe for a single LLM provider.
//!
//! Backs `POST /v1/admin/providers/{id}/probe`: given one provider row, fire a
//! minimal chat request at **each model it advertises** and report which ones
//! actually responded. The result feeds the chat model picker so an operator
//! only ever sees models that genuinely connect (no more picking a model that
//! 401s/404s because the provider's declared list was a guess).
//!
//! Why a transient single-provider router rather than the running one:
//!   - the live `LlmRouter` is built once at boot, so a just-edited key/endpoint
//!     wouldn't be reflected; and
//!   - the live router walks a *fallback chain*, so "model X works" could be a
//!     false positive served by some *other* provider. Building a router from
//!     `[provider]` alone pins every probe to THIS provider.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use xiaoguai_types::LlmProvider;

use crate::build::{build_router, OsEnvResolver};
use crate::router::{LlmRouter, ResolveCtx};
use crate::types::{ChatRequest, Message};

/// Per-model probe outcome.
#[derive(Debug, Clone, Serialize)]
pub struct ModelProbe {
    /// The model id that was probed (as advertised by the provider).
    pub model: String,
    /// `true` when the provider accepted the request and started a response.
    pub ok: bool,
    /// Human-readable failure reason when `ok` is false; omitted on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Round-trip latency to the first streamed chunk (or to the failure).
    pub latency_ms: u64,
}

/// How long a single model probe may take before it's deemed unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// Cap concurrent in-flight probes so a many-model provider can't open a
/// connection per model all at once.
const PROBE_CONCURRENCY: usize = 4;
/// Tiny generation cap — we only need to know the call is accepted, not get a
/// real answer, so keep the token spend per probe negligible.
const PROBE_MAX_TOKENS: u32 = 16;

/// Probe every model `provider` advertises, concurrently (bounded), preserving
/// the declared order in the returned vector.
///
/// `provider.api_key` must already be in cleartext (the repository reveals it on
/// read) — `build_router` reads it directly. Never returns `Err`: a model that
/// can't connect is captured as `ok: false` with its `error`.
pub async fn probe_provider(provider: &LlmProvider) -> Vec<ModelProbe> {
    let (router, _report) = build_router(std::slice::from_ref(provider), &OsEnvResolver);
    let router = Arc::new(router);

    futures::stream::iter(provider.models.clone().into_iter().map(|model| {
        let router = Arc::clone(&router);
        async move { probe_one(&router, model).await }
    }))
    .buffered(PROBE_CONCURRENCY)
    .collect()
    .await
}

/// Probe a single model against an already-built single-provider router.
async fn probe_one(router: &LlmRouter, model: String) -> ModelProbe {
    let start = Instant::now();
    let mut req = ChatRequest::new(model.clone(), vec![Message::user("ping")]);
    req.max_tokens = Some(PROBE_MAX_TOKENS);

    // Success = the initial call is accepted AND the first chunk isn't an error.
    // A provider that returns HTTP 200 then streams an error event still fails.
    let outcome = tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut stream = router.chat_stream(ResolveCtx::default(), req).await?;
        match stream.next().await {
            Some(Err(e)) => Err(e),
            Some(Ok(_)) | None => Ok(()),
        }
    })
    .await;

    let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(Ok(())) => ModelProbe {
            model,
            ok: true,
            error: None,
            latency_ms,
        },
        Ok(Err(e)) => ModelProbe {
            model,
            ok: false,
            error: Some(e.to_string()),
            latency_ms,
        },
        Err(_) => ModelProbe {
            model,
            ok: false,
            error: Some(format!(
                "probe timed out after {}s",
                PROBE_TIMEOUT.as_secs()
            )),
            latency_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use xiaoguai_types::{ProviderId, ProviderKind};

    fn provider(models: &[&str]) -> LlmProvider {
        LlmProvider {
            id: ProviderId::from("prov_test".to_string()),
            name: "test".to_string(),
            kind: ProviderKind::Ollama,
            // Unroutable port — every probe fails fast with a network error,
            // which is exactly the "not connectable" path we want to assert
            // without reaching the network.
            endpoint: "http://127.0.0.1:1".to_string(),
            models: models.iter().map(|m| (*m).to_string()).collect(),
            default_for_models: Vec::new(),
            verified_models: None,
            fallback_order: 1,
            api_key_env: None,
            api_key: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            cost_per_1k_input_usd: None,
            cost_per_1k_output_usd: None,
        }
    }

    #[tokio::test]
    async fn probes_each_model_in_declared_order() {
        let res = probe_provider(&provider(&["a", "b", "c"])).await;
        let models: Vec<&str> = res.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn unreachable_endpoint_reports_not_ok_with_error() {
        let res = probe_provider(&provider(&["a"])).await;
        assert_eq!(res.len(), 1);
        assert!(!res[0].ok);
        assert!(res[0].error.is_some());
    }

    #[tokio::test]
    async fn empty_model_list_yields_empty_result() {
        let res = probe_provider(&provider(&[])).await;
        assert!(res.is_empty());
    }
}
