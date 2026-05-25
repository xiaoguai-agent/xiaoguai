//! Repository and attribution traits for the task board subsystem.
//!
//! [`TaskBoardRepository`] is the primary storage abstraction.  Business
//! logic depends on this trait; concrete implementations live in [`crate::mem`]
//! (tests) and [`crate::pg`] (production Postgres).
//!
//! [`OutcomeAttribution`] is a thin adapter that routes column-transition
//! events into the existing [`xiaoguai_audit::OutcomeRecorder`] pipeline.
//! Every `update_task_column` call produces one attributed outcome event,
//! making the card lifecycle queryable via the outcome-telemetry subsystem
//! without bespoke instrumentation.

use async_trait::async_trait;
use uuid::Uuid;

use xiaoguai_audit::OutcomeRecorder;

use crate::types::{Board, Column, CreateBoardRequest, CreateTaskRequest, Task, TaskStateLogEntry};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum TaskError {
    #[error("task backend: {0}")]
    Backend(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("transition forbidden: cannot move from {from} to {to}")]
    ForbiddenTransition { from: Column, to: Column },
}

// ---------------------------------------------------------------------------
// TaskBoardRepository
// ---------------------------------------------------------------------------

/// Storage abstraction for the task board subsystem.
///
/// All methods are `async` and return `Result<_, TaskError>`.
/// Implementations must be `Send + Sync` so they can be shared across tokio
/// tasks in the dispatcher loop.
#[async_trait]
pub trait TaskBoardRepository: Send + Sync {
    // ---- Boards --------------------------------------------------------

    /// Return all boards for a tenant, ordered by `name`.
    async fn list_boards(&self, tenant_id: Uuid) -> Result<Vec<Board>, TaskError>;

    /// Create a new board.  Exactly one board per tenant may be the default
    /// (the repository enforces this via a unique partial index).
    async fn create_board(&self, req: CreateBoardRequest) -> Result<Board, TaskError>;

    // ---- Tasks ---------------------------------------------------------

    /// Return all tasks on a board, optionally filtered to a single column.
    /// Results are ordered by `priority DESC, created_at ASC`.
    async fn list_tasks(
        &self,
        board_id: Uuid,
        column: Option<Column>,
    ) -> Result<Vec<Task>, TaskError>;

    /// Create a new task.  The card starts in `column` (default `Triage`).
    async fn create_task(&self, req: CreateTaskRequest) -> Result<Task, TaskError>;

    /// Move a task to a new column, recording the transition in
    /// `task_state_log`.
    ///
    /// `actor` is the agent ID, user ID, or `"system"`.
    /// `reason` is optional context (e.g. "assigned by dispatcher").
    async fn update_task_column(
        &self,
        task_id: Uuid,
        new_column: Column,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<Task, TaskError>;

    /// Dispatch the next READY task on a board.
    ///
    /// Picks the highest-priority READY card (by `priority DESC, created_at
    /// ASC`), moves it to RUNNING, sets `assignee_agent`, and returns it.
    /// Returns `Ok(None)` when the READY queue is empty.
    async fn dispatch_next_ready(
        &self,
        board_id: Uuid,
        agent_id: &str,
    ) -> Result<Option<Task>, TaskError>;

    /// Move a RUNNING task to BLOCKED, recording the reason.
    async fn block_task(
        &self,
        task_id: Uuid,
        actor: &str,
        reason: &str,
    ) -> Result<Task, TaskError>;

    /// Return the full state-transition history for a task, ordered by
    /// `occurred_at ASC`.
    async fn get_task_history(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<TaskStateLogEntry>, TaskError>;
}

// ---------------------------------------------------------------------------
// OutcomeAttribution
// ---------------------------------------------------------------------------

/// Routes every card column-transition into the existing outcome-telemetry
/// pipeline so that the full card lifecycle (`TRIAGE → … → DONE`) is
/// queryable via the Outcomes dashboard without separate instrumentation.
///
/// Implementors call [`OutcomeRecorder::record`] with:
/// - `kind = "task_transition"`
/// - `value = 1.0` (one transition event)
/// - `metadata` containing `{ task_id, board_id, from_col, to_col, actor }`
#[async_trait]
pub trait OutcomeAttribution: Send + Sync {
    /// Record one column-transition as a `task_transition` outcome event.
    ///
    /// `recorder` is the shared [`OutcomeRecorder`] instance injected from
    /// the application (e.g. `PgOutcomeRecorder` in production, or
    /// `InMemoryOutcomeRecorder` in tests).
    async fn attribute_transition(
        &self,
        recorder: &dyn OutcomeRecorder,
        tenant_id: &str,
        task_id: Uuid,
        board_id: Uuid,
        from_col: Option<Column>,
        to_col: Column,
        actor: &str,
    ) -> Result<(), xiaoguai_audit::OutcomeError>;
}

// ---------------------------------------------------------------------------
// Default implementation of OutcomeAttribution
// ---------------------------------------------------------------------------

/// Default [`OutcomeAttribution`] that formats the metadata and delegates to
/// the injected recorder.
///
/// This struct is cheap to construct and has no state; wire it once at
/// application startup.
#[derive(Debug, Default, Clone)]
pub struct DefaultOutcomeAttribution;

#[async_trait]
impl OutcomeAttribution for DefaultOutcomeAttribution {
    async fn attribute_transition(
        &self,
        recorder: &dyn OutcomeRecorder,
        tenant_id: &str,
        task_id: Uuid,
        board_id: Uuid,
        from_col: Option<Column>,
        to_col: Column,
        actor: &str,
    ) -> Result<(), xiaoguai_audit::OutcomeError> {
        let metadata = serde_json::json!({
            "task_id":  task_id.to_string(),
            "board_id": board_id.to_string(),
            "from_col": from_col.map(|c| c.as_str()),
            "to_col":   to_col.as_str(),
            "actor":    actor,
        });
        recorder
            .record(
                tenant_id,
                None,
                actor,
                "task_transition",
                1.0,
                Some("count"),
                Some(&format!(
                    "task {task_id} → {to_col}"
                )),
                metadata,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_audit::InMemoryOutcomeRecorder;

    #[tokio::test]
    async fn default_attribution_records_transition() {
        let recorder = InMemoryOutcomeRecorder::new();
        let attr = DefaultOutcomeAttribution;
        let task_id  = Uuid::new_v4();
        let board_id = Uuid::new_v4();

        attr.attribute_transition(
            &recorder,
            "tenant-1",
            task_id,
            board_id,
            Some(Column::Ready),
            Column::Running,
            "dispatcher",
        )
        .await
        .unwrap();

        let snap = recorder.snapshot();
        assert_eq!(snap.len(), 1);
        let rec = &snap[0];
        assert_eq!(rec.kind, "task_transition");
        assert!((rec.value - 1.0).abs() < f64::EPSILON);
        assert_eq!(rec.metadata["to_col"], "running");
        assert_eq!(rec.metadata["from_col"], "ready");
        assert_eq!(rec.metadata["actor"], "dispatcher");
    }

    #[tokio::test]
    async fn default_attribution_no_from_col() {
        let recorder = InMemoryOutcomeRecorder::new();
        let attr = DefaultOutcomeAttribution;

        attr.attribute_transition(
            &recorder,
            "t",
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
            Column::Triage,
            "system",
        )
        .await
        .unwrap();

        let snap = recorder.snapshot();
        assert_eq!(snap[0].metadata["from_col"], serde_json::Value::Null);
        assert_eq!(snap[0].metadata["to_col"], "triage");
    }
}
