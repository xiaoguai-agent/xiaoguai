//! `LlmRouter` — picks one of N registered backends per request.
//!
//! Resolution order (first match wins):
//!
//!   1. `explicit_provider` set on the [`ResolveCtx`].
//!   2. System default for `req.model`.
//!   3. The `fallback_order` chain (used both as the default when no defaults
//!      hit and as the chain to walk when an earlier candidate's *initial*
//!      `chat_stream` call returns an error.
//!
//! Once a stream has yielded its first chunk we never failover: the caller has
//! already started consuming output and re-issuing the request would produce
//! duplicated content. Errors after that point propagate to the caller.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::warn;
use xiaoguai_types::{ProviderId, SessionId, UserId};

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::breaker::Breakers;
use crate::types::ChatRequest;
use crate::usage::{record_on_done, UsageRecord, UsageSink};

/// Static routing configuration. Mutating the router at runtime (e.g. after
/// `xiaoguai provider register`) currently means rebuilding it; refresh
/// support lands with the API server in v0.5.5.
#[derive(Debug, Clone, Default)]
pub struct RouterConfig {
    /// `model_name -> provider` used when nothing more specific resolves.
    pub system_default_for_model: HashMap<String, ProviderId>,
    /// Providers walked in order when nothing more specific resolves and when
    /// an earlier candidate fails its initial call.
    pub fallback_order: Vec<ProviderId>,
    /// Model used when a request omits one (empty `ChatRequest::model`). Set by
    /// `build_router` to the primary (lowest `fallback_order`) provider's first
    /// model — so a single-provider deployment "just works" without `--model`,
    /// and lowering a provider's `fallback_order` makes its model the default.
    pub default_model: Option<String>,
}

/// Per-request context controlling routing resolution and usage attribution.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveCtx<'a> {
    pub explicit_provider: Option<&'a ProviderId>,
    pub user_id: Option<&'a UserId>,
    pub session_id: Option<&'a SessionId>,
    pub request_id: Option<&'a str>,
}

pub struct LlmRouter {
    backends: HashMap<ProviderId, Arc<dyn LlmBackend>>,
    config: RouterConfig,
    usage_sink: Option<Arc<dyn UsageSink>>,
    breakers: Option<Breakers>,
}

impl std::fmt::Debug for LlmRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmRouter")
            .field("backends", &self.backends.keys().collect::<Vec<_>>())
            .field("config", &self.config)
            .field("usage_sink", &self.usage_sink.is_some())
            .field("breakers", &self.breakers.is_some())
            .finish()
    }
}

impl LlmRouter {
    #[must_use]
    pub fn new(backends: HashMap<ProviderId, Arc<dyn LlmBackend>>, config: RouterConfig) -> Self {
        Self {
            backends,
            config,
            usage_sink: None,
            breakers: None,
        }
    }

    /// The model substituted when a request omits one. `None` when no provider
    /// with at least one model registered a backend.
    #[must_use]
    pub fn default_model(&self) -> Option<&str> {
        self.config.default_model.as_deref()
    }

    /// Attach a usage sink. The router will emit one record per successful
    /// stream (on `done: true`). Returns `self` for builder-style chaining.
    #[must_use]
    pub fn with_usage_sink(mut self, sink: Arc<dyn UsageSink>) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    /// Attach a circuit-breaker pool. Candidates whose breaker is `Open` are
    /// skipped during fallback walking; their breaker is recorded on
    /// success / initial-call failure.
    #[must_use]
    pub fn with_breakers(mut self, breakers: Breakers) -> Self {
        self.breakers = Some(breakers);
        self
    }

    /// Stream a chat completion. Walks the candidate list returned by
    /// [`Self::resolve`] until one backend's initial call succeeds.
    pub async fn chat_stream(
        &self,
        ctx: ResolveCtx<'_>,
        mut req: ChatRequest,
    ) -> Result<ChatStream, LlmError> {
        // An empty model means "use the deployment default" — the primary
        // (lowest `fallback_order`) provider's first model. Lets CLI/API callers
        // omit the model when there's an obvious default (single provider, or a
        // provider promoted via `fallback_order`).
        if req.model.trim().is_empty() {
            match &self.config.default_model {
                Some(m) => req.model = m.clone(),
                None => {
                    return Err(LlmError::NoProvider(
                        "no model specified and no default model is configured; \
                         pass an explicit model (e.g. --model MiniMax-M2)"
                            .into(),
                    ))
                }
            }
        }
        let candidates = self.resolve(ctx, &req);
        if candidates.is_empty() {
            return Err(LlmError::NoProvider(
                "no backend matched explicit/default/fallback rules".into(),
            ));
        }

        let mut last_err: Option<LlmError> = None;
        for provider_id in candidates {
            if let Some(b) = &self.breakers {
                if !b.allows_call(&provider_id) {
                    warn!(provider = %provider_id, "circuit breaker open; skipping");
                    continue;
                }
            }
            let Some(backend) = self.backends.get(&provider_id) else {
                warn!(provider = %provider_id, "candidate in config but no backend instance registered");
                continue;
            };
            // Time the provider dispatch on the hot path: emits the
            // `xiaoguai_llm_call_duration_seconds{provider,model}` histogram +
            // an `llm.call` span. No-op until `init_prometheus` ran.
            match xiaoguai_observability::instrument_llm_call!(
                provider_id.as_str(),
                req.model.as_str(),
                backend.chat_stream(req.clone())
            ) {
                Ok(stream) => {
                    if let Some(b) = &self.breakers {
                        b.record_success(&provider_id);
                    }
                    let final_stream = match &self.usage_sink {
                        Some(sink) => {
                            let template = UsageRecord {
                                ts: Utc::now(),
                                user_id: ctx.user_id.cloned(),
                                session_id: ctx.session_id.cloned(),
                                provider_id: provider_id.clone(),
                                model: req.model.clone(),
                                prompt_tokens: None,
                                completion_tokens: None,
                                total_tokens: None,
                                request_id: ctx.request_id.map(str::to_string),
                            };
                            record_on_done(stream, Arc::clone(sink), template)
                        }
                        None => stream,
                    };
                    return Ok(final_stream);
                }
                Err(e) => {
                    if let Some(b) = &self.breakers {
                        b.record_failure(&provider_id);
                    }
                    warn!(provider = %provider_id, error = %e, "backend failed; trying next");
                    last_err = Some(e);
                }
            }
        }

        Err(match last_err {
            Some(e) => LlmError::NoProvider(format!("all candidates failed; last error: {e}")),
            None => LlmError::NoProvider("no candidate backend was registered".into()),
        })
    }

    /// Build the ordered list of provider candidates for this request. Pure
    /// function of `(config, ctx, req.model)` — used both by `chat_stream`
    /// and by tests.
    #[must_use]
    pub fn resolve(&self, ctx: ResolveCtx<'_>, req: &ChatRequest) -> Vec<ProviderId> {
        let mut out: Vec<ProviderId> = Vec::new();
        let push = |p: ProviderId, sink: &mut Vec<ProviderId>| {
            if !sink.contains(&p) {
                sink.push(p);
            }
        };

        if let Some(p) = ctx.explicit_provider {
            push(p.clone(), &mut out);
        }

        if let Some(p) = self.config.system_default_for_model.get(&req.model) {
            push(p.clone(), &mut out);
        }

        for p in &self.config.fallback_order {
            push(p.clone(), &mut out);
        }

        out
    }
}

/// Drop-in `LlmBackend` impl for `LlmRouter` so callers that hold an
/// `Arc<dyn LlmBackend>` (e.g. `AppState.backend` from xiaoguai-api,
/// `ReactAgent::new`) can be handed a router transparently. Routing
/// resolves system defaults + fallback via `ResolveCtx::default()`.
#[async_trait]
impl LlmBackend for LlmRouter {
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.chat_stream(ResolveCtx::default(), req).await
    }

    fn name(&self) -> &'static str {
        "router"
    }
}
