//! Domain types for the task board subsystem.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Column enum
// ---------------------------------------------------------------------------

/// The six columns of the Kanban board, matching the CHECK constraint in
/// migration 0018.  Variants are lowercase for DB and wire compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Triage,
    Todo,
    Ready,
    Running,
    Blocked,
    Done,
}

impl Column {
    /// Database / wire representation (lowercase, matching the DB CHECK).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Todo => "todo",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
        }
    }

    /// Parse the lowercase DB representation.
    ///
    /// Returns `None` for unrecognised strings.
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "triage" => Self::Triage,
            "todo" => Self::Todo,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => return None,
        })
    }

    /// Returns `true` when the column is the terminal DONE state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done)
    }
}

impl std::fmt::Display for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Board
// ---------------------------------------------------------------------------

/// A single Kanban board, scoped to a tenant.
///
/// Boards organise tasks for one team, pack, or environment.  A tenant may
/// have many boards; exactly one can be marked `default_board`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Board {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub default_board: bool,
    pub dispatch_policy: String,
    pub pool_size: i32,
    pub created_at: DateTime<Utc>,
}

/// Input for [`crate::traits::TaskBoardRepository::create_board`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBoardRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub default_board: bool,
    pub dispatch_policy: Option<String>,
    pub pool_size: Option<i32>,
}

// ---------------------------------------------------------------------------
// Task (card)
// ---------------------------------------------------------------------------

/// A single task card on the board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub board_id: Uuid,
    pub column: Column,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    pub assignee_agent: Option<String>,
    pub parent_task_id: Option<Uuid>,
    pub blocked_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for [`crate::traits::TaskBoardRepository::create_task`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub board_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    /// 0–255; higher = more urgent.  Defaults to 128 when `None`.
    pub priority: Option<i32>,
    pub column: Option<Column>,
    pub assignee_agent: Option<String>,
    pub parent_task_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// TaskStateLogEntry
// ---------------------------------------------------------------------------

/// One append-only row in `task_state_log`.
///
/// The sequence of entries for a task IS the outcome-attribution chain
/// described in ADR-0019.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateLogEntry {
    pub id: i64,
    pub task_id: Uuid,
    pub from_column: Option<Column>,
    pub to_column: Column,
    /// Agent ID, user ID, or `"system"` for automated transitions.
    pub actor: String,
    pub reason: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_round_trip_all_variants() {
        let variants = [
            Column::Triage,
            Column::Todo,
            Column::Ready,
            Column::Running,
            Column::Blocked,
            Column::Done,
        ];
        for col in variants {
            let s = col.as_str();
            assert_eq!(Column::from_str(s), Some(col), "round-trip failed for {s}");
        }
    }

    #[test]
    fn column_from_str_unknown_returns_none() {
        assert!(Column::from_str("in_progress").is_none());
        assert!(Column::from_str("").is_none());
    }

    #[test]
    fn column_is_terminal_only_done() {
        assert!(Column::Done.is_terminal());
        assert!(!Column::Running.is_terminal());
        assert!(!Column::Blocked.is_terminal());
        assert!(!Column::Triage.is_terminal());
    }

    #[test]
    fn column_display() {
        assert_eq!(Column::Running.to_string(), "running");
        assert_eq!(Column::Triage.to_string(), "triage");
    }

    #[test]
    fn board_serialises_to_json() {
        let board = Board {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            name: "ops".into(),
            default_board: true,
            dispatch_policy: "fifo".into(),
            pool_size: 5,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&board).unwrap();
        let back: Board = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "ops");
        assert!(back.default_board);
    }

    #[test]
    fn task_state_log_entry_serialises() {
        let entry = TaskStateLogEntry {
            id: 1,
            task_id: Uuid::new_v4(),
            from_column: Some(Column::Ready),
            to_column: Column::Running,
            actor: "dispatcher".into(),
            reason: None,
            occurred_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"running\""));
        assert!(json.contains("\"ready\""));
    }
}
