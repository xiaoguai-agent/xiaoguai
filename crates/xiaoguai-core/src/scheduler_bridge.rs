//! v0.12.0 scheduler bridge — wires `xiaoguai-scheduler` into the
//! operator binary.
//!
//! Three small pieces live here so the cycle-breaking story stays
//! visible:
//!
//! 1. [`WebhookSourceAdapter`] — implements `xiaoguai_api::WebhookPusher`
//!    by forwarding to a `xiaoguai_scheduler::WebhookSource`. This is
//!    the bridge that lets `xiaoguai-api` (which can't depend on
//!    `xiaoguai-scheduler` — see `crates/xiaoguai-api/src/scheduler.rs`)
//!    push events into the scheduler at runtime.
//!
//! 2. [`PgSchedulerAuditAppender`] — implements
//!    `xiaoguai_scheduler::AuditAppender` by forwarding to the
//!    `PgAuditSink` already wired in v0.6.5. Keeps every scheduler-driven
//!    run in the HMAC audit chain alongside REST and IM traffic.
//!
//! 3. [`build_runtime_ctx`] — assembles the `RuntimeContext` the
//!    `RuntimeJobExecutor` runs against. Shares the backend + toolbox +
//!    agent defaults already on `AppState`.

use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::scheduler::{WebhookPushError, WebhookPusher};
use xiaoguai_audit::{chain::sink::PgAuditSink, AuditEntry};
use xiaoguai_llm::LlmBackend;
use xiaoguai_runtime::RuntimeContext;
use xiaoguai_scheduler::{AuditAppender, WebhookSource};

pub struct WebhookSourceAdapter {
    source: Arc<WebhookSource>,
}

impl WebhookSourceAdapter {
    #[must_use]
    pub fn new(source: Arc<WebhookSource>) -> Self {
        Self { source }
    }
}

#[async_trait]
impl WebhookPusher for WebhookSourceAdapter {
    async fn push(
        &self,
        route_id: &str,
        detail: serde_json::Value,
    ) -> Result<usize, WebhookPushError> {
        self.source
            .push(route_id, detail)
            .await
            .map_err(|e| WebhookPushError::Backend(e.to_string()))
    }
}

pub struct PgSchedulerAuditAppender {
    sink: Arc<PgAuditSink>,
}

impl PgSchedulerAuditAppender {
    #[must_use]
    pub fn new(sink: Arc<PgAuditSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl AuditAppender for PgSchedulerAuditAppender {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.sink
            .append(entry)
            .await
            .map(|_stored| ())
            .map_err(|e| e.to_string())
    }
}

#[must_use]
pub fn build_runtime_ctx(
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    agent_defaults: AgentConfig,
) -> Arc<RuntimeContext> {
    Arc::new(RuntimeContext::new(backend, toolbox, agent_defaults))
}
