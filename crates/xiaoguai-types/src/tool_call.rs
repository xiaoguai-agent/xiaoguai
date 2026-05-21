//! Tool call execution lifecycle types.

use crate::ids::ToolCallId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub mcp_server: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub status: ToolCallStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl ToolCall {
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        let started = self.started_at;
        let completed = self.completed_at?;
        let delta = completed - started;
        delta.to_std().ok()
    }
}
