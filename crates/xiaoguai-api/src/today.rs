//! v0.11.1 — audit-first console substrate.
//!
//! `GET /v1/admin/today` is a composite read across chat sessions, IM
//! sessions, and scheduled job runs, sorted by timestamp descending. The
//! console makes this the default landing pane, inverting the chat-first
//! default of every competitor (roadmap §1 + §3 v0.11.1).
//!
//! We keep `xiaoguai-api` free of a concrete-storage dependency the same
//! way `AuditReader` does: define a `TodayReader` trait here, ship a
//! `StaticTodayReader` for route tests, and let `xiaoguai-core` (or any
//! caller) wire a PG-backed adapter at boot.
//!
//! `TodayItem` is internally tagged with `kind` so it round-trips as one
//! union in TypeScript without bespoke deserialization.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum TodayError {
    #[error("today backend: {0}")]
    Backend(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// One row in the unified timeline. `serde(tag = "kind")` mirrors the
/// TS discriminated-union pattern used elsewhere in `@xiaoguai/shared`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TodayItem {
    Chat {
        ts: DateTime<Utc>,
        session_id: String,
        tenant_id: String,
        user_id: String,
        started_at: DateTime<Utc>,
        last_message_preview: Option<String>,
        message_count: i64,
        tool_count: i64,
    },
    Im {
        ts: DateTime<Utc>,
        session_id: String,
        tenant_id: String,
        provider: String,
        chat_id: String,
        started_at: DateTime<Utc>,
        last_message_preview: Option<String>,
        message_count: i64,
    },
    Scheduled {
        ts: DateTime<Utc>,
        job_id: String,
        tenant_id: Option<String>,
        run_id: i64,
        attempt: i32,
        status: String,
        fired_at: DateTime<Utc>,
        output_preview: Option<String>,
        error_message: Option<String>,
        /// Populated when the underlying run originated from a
        /// `Trigger::Proactive` fire (v0.10.2). The reason is the field
        /// the audit-first console highlights first.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl TodayItem {
    /// Sort key — most recent first.
    #[must_use]
    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            Self::Chat { ts, .. } | Self::Im { ts, .. } | Self::Scheduled { ts, .. } => *ts,
        }
    }

    #[must_use]
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Chat { .. } => "chat",
            Self::Im { .. } => "im",
            Self::Scheduled { .. } => "scheduled",
        }
    }
}

/// Filter knobs forwarded to the backing reader. `since` is an inclusive
/// lower bound on `ts`; `kind` restricts to a single source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodayQuery {
    pub limit: i64,
    pub since: Option<DateTime<Utc>>,
    pub kind: Option<TodayKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodayKind {
    Chat,
    Im,
    Scheduled,
}

impl TodayKind {
    #[must_use]
    pub fn matches(self, item: &TodayItem) -> bool {
        matches!(
            (self, item),
            (Self::Chat, TodayItem::Chat { .. })
                | (Self::Im, TodayItem::Im { .. })
                | (Self::Scheduled, TodayItem::Scheduled { .. })
        )
    }
}

#[async_trait]
pub trait TodayReader: Send + Sync {
    /// Return up to `query.limit` items across all three sources, sorted
    /// by `ts` descending. Implementations are responsible for the merge
    /// — the route handler does not re-sort.
    async fn list(&self, query: TodayQuery) -> Result<Vec<TodayItem>, TodayError>;
}

/// In-memory `TodayReader` for route tests and dev mode. Holds a fixed
/// list and filters / sorts on read so individual tests can hand-craft
/// the timeline they want to assert against.
#[derive(Debug, Default, Clone)]
pub struct StaticTodayReader {
    pub items: Vec<TodayItem>,
}

impl StaticTodayReader {
    #[must_use]
    pub fn with_items(items: Vec<TodayItem>) -> Self {
        Self { items }
    }
}

#[async_trait]
impl TodayReader for StaticTodayReader {
    async fn list(&self, query: TodayQuery) -> Result<Vec<TodayItem>, TodayError> {
        if query.limit < 0 {
            return Err(TodayError::InvalidArgument("limit must be >= 0".into()));
        }
        let take = usize::try_from(query.limit).unwrap_or(usize::MAX);
        let mut rows: Vec<TodayItem> = self
            .items
            .iter()
            .filter(|it| query.since.is_none_or(|s| it.ts() >= s))
            .filter(|it| query.kind.is_none_or(|k| k.matches(it)))
            .cloned()
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.ts()));
        rows.truncate(take);
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(ts: DateTime<Utc>, tenant: &str) -> TodayItem {
        TodayItem::Chat {
            ts,
            session_id: "sess_c".into(),
            tenant_id: tenant.into(),
            user_id: "u".into(),
            started_at: ts,
            last_message_preview: Some("hi".into()),
            message_count: 1,
            tool_count: 0,
        }
    }

    fn im(ts: DateTime<Utc>) -> TodayItem {
        TodayItem::Im {
            ts,
            session_id: "sess_i".into(),
            tenant_id: "ten".into(),
            provider: "feishu".into(),
            chat_id: "oc".into(),
            started_at: ts,
            last_message_preview: None,
            message_count: 3,
        }
    }

    fn sched(ts: DateTime<Utc>, run_id: i64) -> TodayItem {
        TodayItem::Scheduled {
            ts,
            job_id: "job_a".into(),
            tenant_id: Some("ten".into()),
            run_id,
            attempt: 1,
            status: "succeeded".into(),
            fired_at: ts,
            output_preview: Some("ok".into()),
            error_message: None,
            reason: None,
        }
    }

    #[tokio::test]
    async fn static_reader_sorts_desc_and_caps_limit() {
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::seconds(60);
        let t2 = t0 + chrono::Duration::seconds(120);
        let reader = StaticTodayReader::with_items(vec![chat(t0, "ten"), sched(t2, 1), im(t1)]);
        let got = reader
            .list(TodayQuery {
                limit: 10,
                since: None,
                kind: None,
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].kind_str(), "scheduled");
        assert_eq!(got[1].kind_str(), "im");
        assert_eq!(got[2].kind_str(), "chat");

        let capped = reader
            .list(TodayQuery {
                limit: 2,
                since: None,
                kind: None,
            })
            .await
            .unwrap();
        assert_eq!(capped.len(), 2);
    }

    #[tokio::test]
    async fn static_reader_filters_by_kind_and_since() {
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::seconds(60);
        let reader =
            StaticTodayReader::with_items(vec![chat(t0, "ten"), im(t0), sched(t1, 1), im(t1)]);
        let only_im = reader
            .list(TodayQuery {
                limit: 10,
                since: None,
                kind: Some(TodayKind::Im),
            })
            .await
            .unwrap();
        assert_eq!(only_im.len(), 2);
        assert!(only_im.iter().all(|i| i.kind_str() == "im"));

        let recent = reader
            .list(TodayQuery {
                limit: 10,
                since: Some(t1),
                kind: None,
            })
            .await
            .unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|i| i.ts() >= t1));
    }

    #[tokio::test]
    async fn static_reader_rejects_negative_limit() {
        let reader = StaticTodayReader::default();
        let err = reader
            .list(TodayQuery {
                limit: -1,
                since: None,
                kind: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, TodayError::InvalidArgument(_)));
    }

    #[test]
    fn today_item_serializes_with_kind_tag() {
        let it = im(Utc::now());
        let json = serde_json::to_value(&it).unwrap();
        assert_eq!(json["kind"], "im");
        assert_eq!(json["provider"], "feishu");
    }

    #[test]
    fn scheduled_item_omits_reason_when_none() {
        let it = sched(Utc::now(), 7);
        let json = serde_json::to_value(&it).unwrap();
        assert!(json.get("reason").is_none(), "reason should be skipped");
    }
}
