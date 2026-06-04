//! Domain types — `ScheduledJob` + `JobRun`.
//!
//! Mirrors the PG schema introduced in migration `0007_scheduled_jobs.sql`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::retry::RetryPolicy;
use crate::trigger::Trigger;

/// A scheduled job definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub trigger: Trigger,
    /// Free-form payload the executor consumes. Production wiring puts
    /// `{ "prompt": "...", "agent_config": { ... } }` here.
    pub payload: serde_json::Value,
    pub retry_policy: RetryPolicy,
    /// IDs of push sinks to deliver results to, e.g.
    /// `["feishu:chat-x", "inbox:user-1"]`. Empty = no push (logs only).
    pub sinks: Vec<String>,
    pub enabled: bool,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ScheduledJob {
    /// Minimal constructor for tests / repository seeding. Real callers
    /// route through a builder once we add one in v0.10.0.x.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        trigger: Trigger,
        payload: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            trigger,
            payload,
            retry_policy: RetryPolicy::default(),
            sinks: Vec::new(),
            enabled: true,
            next_fire_at: None,
            last_fire_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Lifecycle status of an individual job run.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobRunStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobRunStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One execution attempt for a job.
///
/// A run carrying `attempt > 1` is a retry of the same fire. Each
/// retry gets its own [`JobRun`] row so the audit trail is linear.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobRun {
    pub id: i64,
    pub job_id: String,
    pub status: JobRunStatus,
    pub attempt: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Optional link into the `sessions` table — set when the executor
    /// produced a chat-style transcript. The audit-first console
    /// (v0.11.1) joins via this field.
    pub session_id: Option<String>,
    pub error_message: Option<String>,
    /// Short preview of the final output for console rendering. Full
    /// transcript lives in `sessions`.
    pub output_preview: Option<String>,
    pub created_at: DateTime<Utc>,
}
