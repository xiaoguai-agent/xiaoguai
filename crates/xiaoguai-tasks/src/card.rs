//! Core domain types: [`KanbanCard`], [`CardColumn`], [`Outcome`], and
//! the [`Attribution`] chain recorded on completion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque card identifier.
pub type CardId = Uuid;

/// The four Kanban columns cards move through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CardColumn {
    Ready,
    Running,
    Done,
    Blocked,
}

impl CardColumn {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Running => "RUNNING",
            Self::Done => "DONE",
            Self::Blocked => "BLOCKED",
        }
    }
}

impl std::fmt::Display for CardColumn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single step in the attribution chain — who/what produced this outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attribution {
    /// Human-readable identifier (e.g. `"agent:planner-v2"`, `"user:alice"`).
    pub actor: String,
    /// Role in the chain (e.g. `"executor"`, `"reviewer"`, `"orchestrator"`).
    pub role: String,
    /// When this step was recorded.
    pub at: DateTime<Utc>,
    /// Optional free-form note.
    pub note: Option<String>,
}

impl Attribution {
    #[must_use]
    pub fn new(actor: impl Into<String>, role: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            role: role.into(),
            at: Utc::now(),
            note: None,
        }
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// What a successful executor invocation produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    /// Short summary surfaced in the Done card.
    pub summary: String,
    /// Raw output blob (JSON).
    pub output: serde_json::Value,
    /// Attribution chain — populated by the dispatcher after execution.
    pub attribution_chain: Vec<Attribution>,
}

impl Outcome {
    #[must_use]
    pub fn new(summary: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            summary: summary.into(),
            output,
            attribution_chain: Vec::new(),
        }
    }
}

/// A Kanban card flowing through the dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: CardId,
    pub title: String,
    pub column: CardColumn,
    /// Arbitrary task payload, consumed by the [`TaskExecutor`].
    pub payload: serde_json::Value,
    /// Tenant scoping — opaque string; passed through to attribution.
    pub tenant_id: Option<String>,
    /// How many times execution has been attempted (1-indexed).
    pub attempt: u32,
    /// Human-readable reason when column is [`CardColumn::Blocked`].
    pub blocked_reason: Option<String>,
    /// Execution outcome, populated when column is [`CardColumn::Done`].
    pub outcome: Option<Outcome>,
    /// Wall-clock timestamps.
    pub created_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl KanbanCard {
    /// Create a fresh READY card.
    #[must_use]
    pub fn new(title: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            column: CardColumn::Ready,
            payload,
            tenant_id: None,
            attempt: 0,
            blocked_reason: None,
            outcome: None,
            created_at: Utc::now(),
            claimed_at: None,
            completed_at: None,
        }
    }

    #[must_use]
    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }
}
