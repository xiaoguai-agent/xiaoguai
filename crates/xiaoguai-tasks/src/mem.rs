//! In-memory [`TaskBoardRepository`] implementation for unit tests.
//!
//! All state lives in a `Mutex<Store>`; the store is cheap to clone into
//! `Arc<InMemoryTaskBoardRepository>` and share across tasks — the same
//! pattern used by [`xiaoguai_audit::InMemoryOutcomeRecorder`].
//!
//! This implementation does **not** enforce the unique-default-board constraint
//! (that lives in the DB partial index); it prioritises simplicity for tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use crate::{
    traits::{TaskBoardRepository, TaskError},
    types::{Board, Column, CreateBoardRequest, CreateTaskRequest, Task, TaskStateLogEntry},
};

// ---------------------------------------------------------------------------
// Internal store
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Store {
    boards:    Vec<Board>,
    tasks:     Vec<Task>,
    log:       Vec<TaskStateLogEntry>,
    log_seq:   i64,
}

// ---------------------------------------------------------------------------
// InMemoryTaskBoardRepository
// ---------------------------------------------------------------------------

/// Thread-safe in-memory implementation of [`TaskBoardRepository`].
///
/// Intended for unit tests; not optimised for throughput.
#[derive(Debug, Default, Clone)]
pub struct InMemoryTaskBoardRepository {
    inner: Arc<Mutex<Store>>,
}

impl InMemoryTaskBoardRepository {
    /// Construct an empty repository.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TaskBoardRepository for InMemoryTaskBoardRepository {
    // ---- Boards --------------------------------------------------------

    async fn list_boards(&self, tenant_id: Uuid) -> Result<Vec<Board>, TaskError> {
        let store = self.inner.lock().unwrap();
        let mut boards: Vec<Board> = store
            .boards
            .iter()
            .filter(|b| b.tenant_id == tenant_id)
            .cloned()
            .collect();
        boards.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(boards)
    }

    async fn create_board(&self, req: CreateBoardRequest) -> Result<Board, TaskError> {
        let board = Board {
            id:              Uuid::new_v4(),
            tenant_id:       req.tenant_id,
            name:            req.name,
            default_board:   req.default_board,
            dispatch_policy: req.dispatch_policy.unwrap_or_else(|| "fifo".into()),
            pool_size:       req.pool_size.unwrap_or(5),
            created_at:      Utc::now(),
        };
        let mut store = self.inner.lock().unwrap();
        store.boards.push(board.clone());
        Ok(board)
    }

    // ---- Tasks ---------------------------------------------------------

    async fn list_tasks(
        &self,
        board_id: Uuid,
        column: Option<Column>,
    ) -> Result<Vec<Task>, TaskError> {
        let store = self.inner.lock().unwrap();
        let mut tasks: Vec<Task> = store
            .tasks
            .iter()
            .filter(|t| t.board_id == board_id)
            .filter(|t| column.map_or(true, |c| t.column == c))
            .cloned()
            .collect();
        // priority DESC, created_at ASC
        tasks.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.created_at.cmp(&b.created_at))
        });
        Ok(tasks)
    }

    async fn create_task(&self, req: CreateTaskRequest) -> Result<Task, TaskError> {
        let priority = req.priority.unwrap_or(128);
        if !(0..=255).contains(&priority) {
            return Err(TaskError::InvalidArgument(
                "priority must be 0–255".into(),
            ));
        }
        let task = Task {
            id:             Uuid::new_v4(),
            board_id:       req.board_id,
            column:         req.column.unwrap_or(Column::Triage),
            title:          req.title,
            description:    req.description,
            priority,
            assignee_agent: req.assignee_agent,
            parent_task_id: req.parent_task_id,
            blocked_reason: None,
            created_at:     Utc::now(),
            updated_at:     Utc::now(),
        };
        let mut store = self.inner.lock().unwrap();
        store.tasks.push(task.clone());
        // Record creation event.
        store.log_seq += 1;
        store.log.push(TaskStateLogEntry {
            id:          store.log_seq,
            task_id:     task.id,
            from_column: None,
            to_column:   task.column,
            actor:       "system".into(),
            reason:      Some("created".into()),
            occurred_at: Utc::now(),
        });
        Ok(task)
    }

    async fn update_task_column(
        &self,
        task_id: Uuid,
        new_column: Column,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<Task, TaskError> {
        let mut store = self.inner.lock().unwrap();
        let task = store
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

        let old_column = task.column;
        task.column     = new_column;
        task.updated_at = Utc::now();
        // Clear blocked_reason when leaving BLOCKED.
        if old_column == Column::Blocked && new_column != Column::Blocked {
            task.blocked_reason = None;
        }
        let task_clone = task.clone();

        store.log_seq += 1;
        store.log.push(TaskStateLogEntry {
            id:          store.log_seq,
            task_id,
            from_column: Some(old_column),
            to_column:   new_column,
            actor:       actor.to_owned(),
            reason:      reason.map(ToOwned::to_owned),
            occurred_at: Utc::now(),
        });
        Ok(task_clone)
    }

    async fn dispatch_next_ready(
        &self,
        board_id: Uuid,
        agent_id: &str,
    ) -> Result<Option<Task>, TaskError> {
        let mut store = self.inner.lock().unwrap();
        // Find the highest-priority READY task on this board.
        let candidate = store
            .tasks
            .iter()
            .filter(|t| t.board_id == board_id && t.column == Column::Ready)
            .max_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then(b.created_at.cmp(&a.created_at))
            })
            .map(|t| t.id);

        let Some(task_id) = candidate else {
            return Ok(None);
        };

        let task = store
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .unwrap();

        task.column         = Column::Running;
        task.assignee_agent = Some(agent_id.to_owned());
        task.updated_at     = Utc::now();
        let task_clone = task.clone();

        store.log_seq += 1;
        store.log.push(TaskStateLogEntry {
            id:          store.log_seq,
            task_id,
            from_column: Some(Column::Ready),
            to_column:   Column::Running,
            actor:       agent_id.to_owned(),
            reason:      Some("dispatched".into()),
            occurred_at: Utc::now(),
        });
        Ok(Some(task_clone))
    }

    async fn block_task(
        &self,
        task_id: Uuid,
        actor: &str,
        reason: &str,
    ) -> Result<Task, TaskError> {
        let mut store = self.inner.lock().unwrap();
        let task = store
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

        let old_column      = task.column;
        task.column         = Column::Blocked;
        task.blocked_reason = Some(reason.to_owned());
        task.updated_at     = Utc::now();
        let task_clone = task.clone();

        store.log_seq += 1;
        store.log.push(TaskStateLogEntry {
            id:          store.log_seq,
            task_id,
            from_column: Some(old_column),
            to_column:   Column::Blocked,
            actor:       actor.to_owned(),
            reason:      Some(reason.to_owned()),
            occurred_at: Utc::now(),
        });
        Ok(task_clone)
    }

    async fn get_task_history(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<TaskStateLogEntry>, TaskError> {
        let store = self.inner.lock().unwrap();
        let mut entries: Vec<TaskStateLogEntry> = store
            .log
            .iter()
            .filter(|e| e.task_id == task_id)
            .cloned()
            .collect();
        entries.sort_by_key(|e| e.occurred_at);
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CreateBoardRequest;

    fn make_board_req(tenant_id: Uuid) -> CreateBoardRequest {
        CreateBoardRequest {
            tenant_id,
            name:            "test-board".into(),
            default_board:   false,
            dispatch_policy: None,
            pool_size:       None,
        }
    }

    fn make_task_req(board_id: Uuid) -> CreateTaskRequest {
        CreateTaskRequest {
            board_id,
            title:          "Fix the thing".into(),
            description:    Some("Details here".into()),
            priority:       None,
            column:         None,
            assignee_agent: None,
            parent_task_id: None,
        }
    }

    #[tokio::test]
    async fn create_and_list_boards() {
        let repo = InMemoryTaskBoardRepository::new();
        let tenant = Uuid::new_v4();
        repo.create_board(make_board_req(tenant)).await.unwrap();
        let boards = repo.list_boards(tenant).await.unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].name, "test-board");
        assert_eq!(boards[0].dispatch_policy, "fifo");
        assert_eq!(boards[0].pool_size, 5);
    }

    #[tokio::test]
    async fn list_boards_scoped_to_tenant() {
        let repo = InMemoryTaskBoardRepository::new();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        repo.create_board(make_board_req(t1)).await.unwrap();
        repo.create_board(make_board_req(t2)).await.unwrap();
        assert_eq!(repo.list_boards(t1).await.unwrap().len(), 1);
        assert_eq!(repo.list_boards(t2).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn create_task_lands_in_triage_by_default() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let task  = repo.create_task(make_task_req(board.id)).await.unwrap();
        assert_eq!(task.column, Column::Triage);
        assert_eq!(task.priority, 128);
        assert!(task.blocked_reason.is_none());
    }

    #[tokio::test]
    async fn create_task_with_explicit_column_and_priority() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let req   = CreateTaskRequest {
            board_id:       board.id,
            title:          "Ready task".into(),
            description:    None,
            priority:       Some(200),
            column:         Some(Column::Ready),
            assignee_agent: None,
            parent_task_id: None,
        };
        let task = repo.create_task(req).await.unwrap();
        assert_eq!(task.column, Column::Ready);
        assert_eq!(task.priority, 200);
    }

    #[tokio::test]
    async fn create_task_rejects_out_of_range_priority() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let req   = CreateTaskRequest {
            board_id: board.id,
            title: "Bad".into(),
            description: None,
            priority: Some(256),
            column: None,
            assignee_agent: None,
            parent_task_id: None,
        };
        assert!(matches!(
            repo.create_task(req).await.unwrap_err(),
            TaskError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn list_tasks_filtered_by_column() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();

        // triage (default)
        repo.create_task(make_task_req(board.id)).await.unwrap();

        let req = CreateTaskRequest {
            board_id: board.id,
            title: "Ready task".into(),
            description: None,
            priority: None,
            column: Some(Column::Ready),
            assignee_agent: None,
            parent_task_id: None,
        };
        repo.create_task(req).await.unwrap();

        let all    = repo.list_tasks(board.id, None).await.unwrap();
        let triage = repo.list_tasks(board.id, Some(Column::Triage)).await.unwrap();
        let ready  = repo.list_tasks(board.id, Some(Column::Ready)).await.unwrap();

        assert_eq!(all.len(), 2);
        assert_eq!(triage.len(), 1);
        assert_eq!(ready.len(), 1);
    }

    #[tokio::test]
    async fn update_task_column_records_history() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let task  = repo.create_task(make_task_req(board.id)).await.unwrap();

        repo.update_task_column(task.id, Column::Todo, "user-1", Some("approved"))
            .await
            .unwrap();

        let history = repo.get_task_history(task.id).await.unwrap();
        // Entry 0: creation (triage); Entry 1: triage→todo.
        assert_eq!(history.len(), 2);
        let transition = &history[1];
        assert_eq!(transition.from_column, Some(Column::Triage));
        assert_eq!(transition.to_column, Column::Todo);
        assert_eq!(transition.actor, "user-1");
        assert_eq!(transition.reason.as_deref(), Some("approved"));
    }

    #[tokio::test]
    async fn update_task_column_not_found_returns_error() {
        let repo = InMemoryTaskBoardRepository::new();
        let err  = repo
            .update_task_column(Uuid::new_v4(), Column::Done, "x", None)
            .await
            .unwrap_err();
        assert!(matches!(err, TaskError::NotFound(_)));
    }

    #[tokio::test]
    async fn dispatch_next_ready_picks_highest_priority() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();

        for priority in [100u8, 200u8, 50u8] {
            let req = CreateTaskRequest {
                board_id:       board.id,
                title:          format!("Task p{priority}"),
                description:    None,
                priority:       Some(i32::from(priority)),
                column:         Some(Column::Ready),
                assignee_agent: None,
                parent_task_id: None,
            };
            repo.create_task(req).await.unwrap();
        }

        let dispatched = repo
            .dispatch_next_ready(board.id, "agent-1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(dispatched.column, Column::Running);
        assert_eq!(dispatched.priority, 200);
        assert_eq!(dispatched.assignee_agent.as_deref(), Some("agent-1"));
    }

    #[tokio::test]
    async fn dispatch_next_ready_returns_none_when_queue_empty() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();

        let result = repo
            .dispatch_next_ready(board.id, "agent-1")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn block_task_sets_blocked_reason() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let task  = repo.create_task(make_task_req(board.id)).await.unwrap();

        let blocked = repo
            .block_task(task.id, "agent-2", "needs human approval")
            .await
            .unwrap();

        assert_eq!(blocked.column, Column::Blocked);
        assert_eq!(
            blocked.blocked_reason.as_deref(),
            Some("needs human approval")
        );
    }

    #[tokio::test]
    async fn blocked_reason_cleared_on_column_change() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let task  = repo.create_task(make_task_req(board.id)).await.unwrap();

        repo.block_task(task.id, "agent-2", "needs approval")
            .await
            .unwrap();

        let unblocked = repo
            .update_task_column(task.id, Column::Ready, "human-1", Some("approved"))
            .await
            .unwrap();

        assert_eq!(unblocked.column, Column::Ready);
        assert!(unblocked.blocked_reason.is_none());
    }

    #[tokio::test]
    async fn get_task_history_returns_empty_for_unknown_task() {
        let repo = InMemoryTaskBoardRepository::new();
        let history = repo.get_task_history(Uuid::new_v4()).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn full_lifecycle_history_ordered_by_occurred_at() {
        let repo  = InMemoryTaskBoardRepository::new();
        let board = repo.create_board(make_board_req(Uuid::new_v4())).await.unwrap();
        let task  = repo.create_task(make_task_req(board.id)).await.unwrap();

        repo.update_task_column(task.id, Column::Todo, "u", None).await.unwrap();
        repo.update_task_column(task.id, Column::Ready, "u", None).await.unwrap();
        repo.dispatch_next_ready(board.id, "agent-1").await.unwrap();
        repo.update_task_column(task.id, Column::Done, "agent-1", None).await.unwrap();

        let history = repo.get_task_history(task.id).await.unwrap();
        // created + todo + ready + running + done = 5 entries
        assert_eq!(history.len(), 5);
        assert_eq!(history[0].to_column, Column::Triage);
        assert_eq!(history[4].to_column, Column::Done);
    }
}
