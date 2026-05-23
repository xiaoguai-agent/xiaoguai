//! Webhook source — in-process router from `route_id` to job ids.
//!
//! v0.10.1 keeps the actual HTTP route out of `xiaoguai-api`; that
//! belongs to the operator-wiring slice deferred from v0.10.0 (small
//! `tokio::spawn` change to drive `JobRunner::run_loop` plus a thin
//! axum handler that calls [`WebhookSource::push`]).
//!
//! What ships here is the connector: a registry of
//! `route_id → [job_id]` and a [`WebhookSource::push`] entry point.
//! Whoever fronts it (an axum handler today; tomorrow maybe a gRPC
//! server, an Slack-events webhook, or a CLI command for manual
//! triggering) calls `push` and the source fans out one event per
//! bound job.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::trigger_source::{EventSender, SourceError, TriggerEvent, TriggerSource};

/// One (`route_id`, `job_id`) binding.
#[derive(Debug, Clone)]
pub struct WebhookRoute {
    pub route_id: String,
    pub job_id: String,
}

impl WebhookRoute {
    #[must_use]
    pub fn new(route_id: impl Into<String>, job_id: impl Into<String>) -> Self {
        Self {
            route_id: route_id.into(),
            job_id: job_id.into(),
        }
    }
}

/// In-process webhook source. The HTTP layer calls [`Self::push`]
/// with the `route_id` extracted from the URL plus any payload it
/// wants to surface in audit details.
pub struct WebhookSource {
    routes: Arc<Mutex<HashMap<String, Vec<String>>>>,
    tx: Mutex<Option<EventSender>>,
}

impl WebhookSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            tx: Mutex::new(None),
        }
    }

    /// Register a (`route_id`, `job_id`) binding. Multiple jobs can
    /// share one `route_id`; they all fire when the webhook arrives.
    pub fn add_route(&self, route: WebhookRoute) {
        self.routes
            .lock()
            .entry(route.route_id)
            .or_default()
            .push(route.job_id);
    }

    /// Send a `TriggerEvent` for every job bound to `route_id`.
    ///
    /// Returns the number of events delivered. Returns `Ok(0)` if no
    /// jobs are bound to the route (the HTTP handler can translate
    /// that to a 404). Returns an error if the source hasn't been
    /// started yet or the runner has shut down.
    pub async fn push(
        &self,
        route_id: &str,
        detail: serde_json::Value,
    ) -> Result<usize, SourceError> {
        let tx = {
            let g = self.tx.lock();
            g.clone()
        };
        let Some(tx) = tx else {
            return Err(SourceError::Backend("webhook source not started".into()));
        };
        let job_ids = {
            let g = self.routes.lock();
            g.get(route_id).cloned().unwrap_or_default()
        };
        let mut delivered = 0;
        for job_id in job_ids {
            let ev = TriggerEvent::new(job_id).with_detail(detail.clone());
            tx.send(ev)
                .await
                .map_err(|_| SourceError::Backend("event receiver dropped".into()))?;
            delivered += 1;
        }
        Ok(delivered)
    }

    /// Drop all routes. Diagnostic helper for tests + admin-ui.
    pub fn clear(&self) {
        self.routes.lock().clear();
    }

    /// Snapshot of every registered `route_id` (sorted, stable).
    #[must_use]
    pub fn route_ids(&self) -> Vec<String> {
        let g = self.routes.lock();
        let mut out: Vec<String> = g.keys().cloned().collect();
        out.sort();
        out
    }
}

impl Default for WebhookSource {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WebhookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSource")
            .field("route_count", &self.routes.lock().len())
            .field("started", &self.tx.lock().is_some())
            .finish()
    }
}

#[async_trait]
impl TriggerSource for WebhookSource {
    fn id(&self) -> &'static str {
        "webhook"
    }

    async fn start(&self, tx: EventSender) -> Result<(), SourceError> {
        let mut g = self.tx.lock();
        if g.is_some() {
            return Err(SourceError::AlreadyStarted);
        }
        *g = Some(tx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger_source::event_channel;

    #[tokio::test]
    async fn push_before_start_errors() {
        let src = WebhookSource::new();
        src.add_route(WebhookRoute::new("r1", "j1"));
        let err = src.push("r1", serde_json::Value::Null).await.unwrap_err();
        assert!(matches!(err, SourceError::Backend(_)));
    }

    #[tokio::test]
    async fn push_unknown_route_returns_zero() {
        let src = WebhookSource::new();
        let (tx, _rx) = event_channel();
        src.start(tx).await.unwrap();
        let n = src
            .push("no-such-route", serde_json::Value::Null)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn push_fans_out_to_every_bound_job() {
        let src = WebhookSource::new();
        src.add_route(WebhookRoute::new("deploy", "j-build"));
        src.add_route(WebhookRoute::new("deploy", "j-notify"));
        let (tx, mut rx) = event_channel();
        src.start(tx).await.unwrap();

        let n = src
            .push("deploy", serde_json::json!({"sha": "abc"}))
            .await
            .unwrap();
        assert_eq!(n, 2);

        let mut got: Vec<String> = Vec::new();
        for _ in 0..2 {
            let ev = rx.recv().await.unwrap();
            assert_eq!(ev.detail, serde_json::json!({"sha": "abc"}));
            got.push(ev.job_id);
        }
        got.sort();
        assert_eq!(got, vec!["j-build".to_string(), "j-notify".to_string()]);
    }

    #[tokio::test]
    async fn double_start_errors() {
        let src = WebhookSource::new();
        let (tx, _rx) = event_channel();
        src.start(tx.clone()).await.unwrap();
        let err = src.start(tx).await.unwrap_err();
        assert!(matches!(err, SourceError::AlreadyStarted));
    }
}
