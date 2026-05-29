//! Session-scoped watcher introspection — backs `/v1/watchers/*`.
//!
//! `xiaoguai-watch::WatchRunner` today tracks watchers globally (by
//! `WatchSpec.id`), not per-session, and exposes no introspection — the
//! `run()` method consumes the runner and only emits `WatchEvent`s on its
//! mpsc channel. Extending the runner with session-awareness is real
//! architectural surgery (the `WatchSpec` schema would need a new field;
//! the dedup/scheduler hookup would need to filter by session; etc.) so
//! sprint-10b deliberately keeps that change out of scope.
//!
//! Instead we model the API-layer requirement with its own
//! [`WatcherIntrospector`] trait, mirroring the pattern used by
//! [`crate::today::TodayReader`] and [`crate::usage::UsageReader`]:
//! production wires a concrete adapter when a session-aware
//! `WatchRunner` exists; the static (`Static…`) adapter ships with this
//! crate so that the UI's `<WatchIndicator>` can degrade to "no
//! watchers" without falling all the way to its 404 fallback path.
//!
//! ## Wire shape
//!
//! Matches the existing `XiaoguaiClient.listSessionWatchers /
//! pauseWatcher / resumeWatcher` calls in `frontend/shared/src/index.ts`
//! — three endpoints (`GET /v1/watchers?session_id=…`,
//! `POST /v1/watchers/:id/pause`, `POST /v1/watchers/:id/resume`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Lifecycle state of a watcher. Matches the TS literal type
/// `WatcherStatus` in `frontend/shared/src/index.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatcherStatus {
    Running,
    Paused,
    Error,
}

/// Source category for a watcher row. Mirrors the TS
/// `WatcherSourceType` union — we accept and emit free-form strings
/// to keep room for adapter-specific variants without churning the
/// API contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatcherSourceType {
    Schedule,
    Webhook,
    Manual,
    #[serde(other)]
    Other,
}

/// One row returned by `GET /v1/watchers?session_id=…`. Field shape
/// mirrors the TypeScript `WatcherInfo` interface verbatim so the
/// client doesn't need a translation layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatcherInfo {
    pub id: String,
    pub name: String,
    pub source_type: WatcherSourceType,
    pub last_fired_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: WatcherStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
}

/// Errors surfaced through the HTTP handlers.
#[derive(Debug, Error)]
pub enum WatcherError {
    /// No watcher with the requested ID is known. Maps to 404.
    #[error("watcher not found: {0}")]
    NotFound(String),
    /// Generic backend failure; maps to 500.
    #[error("watcher backend error: {0}")]
    Backend(String),
}

/// API-layer abstraction over a watcher state store. Production wires
/// a concrete adapter that holds shared state with the running
/// `WatchRunner`; the static variant below is enough to unblock the
/// frontend until that adapter ships.
#[async_trait]
pub trait WatcherIntrospector: Send + Sync {
    /// Return all watchers currently bound to `session_id`. An empty
    /// vec is the "no watchers" steady state — not an error.
    ///
    /// # Errors
    /// Returns [`WatcherError::Backend`] if the underlying state store
    /// fails to enumerate watchers.
    async fn list_for_session(&self, session_id: &str) -> Result<Vec<WatcherInfo>, WatcherError>;

    /// Pause a watcher by id. Idempotent — pausing an already-paused
    /// watcher returns `Ok`.
    ///
    /// # Errors
    /// Returns [`WatcherError::NotFound`] when no watcher has the
    /// given id; [`WatcherError::Backend`] for state-store failures.
    async fn pause(&self, watcher_id: &str) -> Result<(), WatcherError>;

    /// Resume a paused / errored watcher.
    ///
    /// # Errors
    /// See [`WatcherIntrospector::pause`].
    async fn resume(&self, watcher_id: &str) -> Result<(), WatcherError>;
}

/// Static introspector that always reports zero watchers — the
/// minimum viable mount that lets `<WatchIndicator>` render an empty
/// state on a 200 response (rather than the 404-fallback path).
///
/// Operators wire this until a session-aware `WatchRunner` ships;
/// pause / resume calls return 404 because there are no watchers to
/// act on.
#[derive(Default, Clone)]
pub struct StaticWatcherIntrospector;

impl StaticWatcherIntrospector {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn arc() -> Arc<dyn WatcherIntrospector> {
        Arc::new(Self)
    }
}

#[async_trait]
impl WatcherIntrospector for StaticWatcherIntrospector {
    async fn list_for_session(&self, _session_id: &str) -> Result<Vec<WatcherInfo>, WatcherError> {
        Ok(Vec::new())
    }

    async fn pause(&self, watcher_id: &str) -> Result<(), WatcherError> {
        Err(WatcherError::NotFound(watcher_id.to_string()))
    }

    async fn resume(&self, watcher_id: &str) -> Result<(), WatcherError> {
        Err(WatcherError::NotFound(watcher_id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_introspector_reports_empty_list() {
        let intro = StaticWatcherIntrospector::new();
        let v = intro.list_for_session("sess_x").await.unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn static_introspector_reports_404_on_pause() {
        let intro = StaticWatcherIntrospector::new();
        let err = intro.pause("w1").await.unwrap_err();
        assert!(matches!(err, WatcherError::NotFound(_)));
    }

    #[tokio::test]
    async fn static_introspector_reports_404_on_resume() {
        let intro = StaticWatcherIntrospector::new();
        let err = intro.resume("w1").await.unwrap_err();
        assert!(matches!(err, WatcherError::NotFound(_)));
    }

    #[test]
    fn watcher_info_serializes_with_camel_case_status_lowercase() {
        // status is rendered as lowercase ("running") — must match the
        // TS WatcherStatus literal union so the client doesn't need a
        // translation layer.
        let info = WatcherInfo {
            id: "w1".into(),
            name: "Nightly".into(),
            source_type: WatcherSourceType::Schedule,
            last_fired_at: None,
            status: WatcherStatus::Running,
            schedule: Some("0 0 * * *".into()),
        };
        let json = serde_json::to_value(info).unwrap();
        assert_eq!(json["status"], "running");
        assert_eq!(json["source_type"], "schedule");
    }
}
