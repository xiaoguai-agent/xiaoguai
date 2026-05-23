//! Push sinks — where job results land.
//!
//! v0.10.0 ships the trait + one stub [`LoggingSink`] that writes the
//! payload to `tracing::info!`. Real sinks (Feishu / Telegram / Email
//! / chat-ui inbox) land in v0.10.3 against the same trait.
//!
//! v0.10.2 adds the `reason: String` field to [`PushPayload`]
//! (roadmap §5.5). Scheduled / reactive fires populate it with empty
//! string by default; proactive fires populate it with the
//! checker-returned reason. Real sinks (v0.10.3) refuse delivery when
//! the originating trigger is proactive *and* `reason.is_empty()` —
//! the field uses `#[serde(default)]` so older persisted rows decode
//! cleanly.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SinkError {
    #[error("delivery failed: {0}")]
    Delivery(String),
    #[error("invalid payload: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushPayload {
    pub job_id: String,
    pub run_id: i64,
    pub tenant_id: Option<String>,
    pub status: String,
    pub fired_at: DateTime<Utc>,
    pub output_preview: Option<String>,
    pub error_message: Option<String>,
    /// Reason this push exists. Empty for scheduled / reactive jobs;
    /// populated with the [`crate::proactive::ProactiveChecker`]
    /// verdict for proactive jobs. Sinks rendering proactive payloads
    /// MUST surface this field — see roadmap §5.5.
    #[serde(default)]
    pub reason: String,
}

#[async_trait]
pub trait PushSink: Send + Sync {
    /// Stable identifier so a `ScheduledJob` can pick this sink by id.
    fn id(&self) -> &str;

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError>;
}

/// Logging sink — writes the payload via `tracing::info!`. Useful for
/// development; in production the supervisor wires the real sinks from
/// the registry.
pub struct LoggingSink {
    id: String,
    captured: Mutex<Vec<PushPayload>>,
}

impl LoggingSink {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            captured: Mutex::new(Vec::new()),
        }
    }

    /// Test-only: read back every payload this sink has received.
    #[must_use]
    pub fn captured(&self) -> Vec<PushPayload> {
        self.captured.lock().clone()
    }
}

impl std::fmt::Debug for LoggingSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggingSink")
            .field("id", &self.id)
            .field("captured_count", &self.captured.lock().len())
            .finish()
    }
}

#[async_trait]
impl PushSink for LoggingSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError> {
        tracing::info!(
            sink = %self.id,
            job_id = %payload.job_id,
            run_id = payload.run_id,
            status = %payload.status,
            "scheduler push"
        );
        self.captured.lock().push(payload.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn logging_sink_captures_payload() {
        let s = LoggingSink::new("inbox");
        let p = PushPayload {
            job_id: "j1".into(),
            run_id: 1,
            tenant_id: Some("t1".into()),
            status: "succeeded".into(),
            fired_at: Utc::now(),
            output_preview: Some("done".into()),
            error_message: None,
            reason: String::new(),
        };
        s.deliver(&p).await.unwrap();
        let cap = s.captured();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0].run_id, 1);
        assert_eq!(s.id(), "inbox");
    }

    #[test]
    fn payload_decodes_without_reason_field_for_back_compat() {
        // Old persisted rows from v0.10.0/v0.10.1 don't have `reason`.
        // Default must kick in so we can still parse them.
        let raw = r#"{
            "job_id": "j1",
            "run_id": 1,
            "tenant_id": null,
            "status": "succeeded",
            "fired_at": "2026-05-23T10:00:00Z",
            "output_preview": null,
            "error_message": null
        }"#;
        let p: PushPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p.reason, "");
    }
}
