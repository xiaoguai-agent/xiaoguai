//! Scheduler integration types — kept here (not in `xiaoguai-scheduler`)
//! to avoid a dependency cycle.
//!
//! `xiaoguai-scheduler` already transitively depends on `xiaoguai-api`
//! (via `xiaoguai-im-feishu` → `xiaoguai-im-gateway` → `xiaoguai-api`),
//! so `xiaoguai-api` cannot depend on `xiaoguai-scheduler` directly.
//! Instead `xiaoguai-api` owns the small trait surface it needs from
//! the scheduler — same pattern as [`crate::audit::AuditReader`] and
//! [`crate::today::TodayReader`].
//!
//! `xiaoguai-core` provides the production impl by wrapping
//! `xiaoguai_scheduler::WebhookSource`.

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WebhookPushError {
    #[error("source backend: {0}")]
    Backend(String),
}

/// Push a reactive trigger event onto the scheduler's event channel.
///
/// `route_id` identifies the bound (route → job) mapping inside the
/// scheduler. `detail` is opaque JSON that lands in the audit row of
/// every fired job under `details.trigger`.
///
/// Returns the count of jobs that were notified. A return of `Ok(0)`
/// means no jobs are bound to `route_id` — the HTTP handler maps that
/// to 404.
#[async_trait]
pub trait WebhookPusher: Send + Sync {
    async fn push(
        &self,
        route_id: &str,
        detail: serde_json::Value,
    ) -> Result<usize, WebhookPushError>;
}
