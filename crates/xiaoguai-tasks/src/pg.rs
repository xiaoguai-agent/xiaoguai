//! Postgres-backed [`TaskBoardRepository`] implementation.
//!
//! Uses the tables created by migration 0016 (`boards`, `tasks`,
//! `task_state_log`).  All queries use `sqlx::query_as!` / `sqlx::query!`
//! with compile-time checked SQL when the `DATABASE_URL` env var is set;
//! otherwise they fall back to the dynamic variants so the crate builds
//! without a live database.
//!
//! This implementation is **not** included in unit test runs — use
//! [`crate::mem::InMemoryTaskBoardRepository`] instead.  Integration tests
//! against a real PG instance (testcontainers) live in `tests/pg_tasks.rs`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    traits::{TaskBoardRepository, TaskError},
    types::{Board, Column, CreateBoardRequest, CreateTaskRequest, Task, TaskStateLogEntry},
};

// ---------------------------------------------------------------------------
// Row types — sqlx FromRow derives
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct BoardRow {
    id:              Uuid,
    tenant_id:       Uuid,
    name:            String,
    default_board:   bool,
    dispatch_policy: String,
    pool_size:       i32,
    created_at:      DateTime<Utc>,
}

impl From<BoardRow> for Board {
    fn from(r: BoardRow) -> Self {
        Self {
            id:              r.id,
            tenant_id:       r.tenant_id,
            name:            r.name,
            default_board:   r.default_board,
            dispatch_policy: r.dispatch_policy,
            pool_size:       r.pool_size,
            created_at:      r.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    id:             Uuid,
    board_id:       Uuid,
    column:         String,
    title:          String,
    description:    Option<String>,
    priority:       i32,
    assignee_agent: Option<String>,
    parent_task_id: Option<Uuid>,
    blocked_reason: Option<String>,
    created_at:     DateTime<Utc>,
    updated_at:     DateTime<Utc>,
}

impl TryFrom<TaskRow> for Task {
    type Error = TaskError;
    fn try_from(r: TaskRow) -> Result<Self, Self::Error> {
        let column = Column::from_str(&r.column).ok_or_else(|| {
            TaskError::Backend(format!("unknown column value in DB: '{}'", r.column))
        })?;
        Ok(Self {
            id:             r.id,
            board_id:       r.board_id,
            column,
            title:          r.title,
            description:    r.description,
            priority:       r.priority,
            assignee_agent: r.assignee_agent,
            parent_task_id: r.parent_task_id,
            blocked_reason: r.blocked_reason,
            created_at:     r.created_at,
            updated_at:     r.updated_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct LogRow {
    id:          i64,
    task_id:     Uuid,
    from_column: Option<String>,
    to_column:   String,
    actor:       String,
    reason:      Option<String>,
    occurred_at: DateTime<Utc>,
}

impl TryFrom<LogRow> for TaskStateLogEntry {
    type Error = TaskError;
    fn try_from(r: LogRow) -> Result<Self, Self::Error> {
        let from_column = r
            .from_column
            .as_deref()
            .map(|s| {
                Column::from_str(s).ok_or_else(|| {
                    TaskError::Backend(format!("unknown from_column in DB: '{s}'"))
                })
            })
            .transpose()?;
        let to_column = Column::from_str(&r.to_column).ok_or_else(|| {
            TaskError::Backend(format!("unknown to_column in DB: '{}'", r.to_column))
        })?;
        Ok(Self {
            id:          r.id,
            task_id:     r.task_id,
            from_column,
            to_column,
            actor:       r.actor,
            reason:      r.reason,
            occurred_at: r.occurred_at,
        })
    }
}

// ---------------------------------------------------------------------------
// Helper: map sqlx::Error → TaskError
// ---------------------------------------------------------------------------

fn db_err(e: sqlx::Error) -> TaskError {
    TaskError::Backend(e.to_string())
}

// ---------------------------------------------------------------------------
// PgTaskBoardRepository
// ---------------------------------------------------------------------------

/// Postgres-backed [`TaskBoardRepository`].
///
/// Construct with [`PgTaskBoardRepository::new`] and a `PgPool` obtained
/// from `xiaoguai-storage`'s connection pool helper.
#[derive(Debug, Clone)]
pub struct PgTaskBoardRepository {
    pool: PgPool,
}

impl PgTaskBoardRepository {
    /// Create a new repository backed by `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TaskBoardRepository for PgTaskBoardRepository {
    // ---- Boards --------------------------------------------------------

    async fn list_boards(&self, tenant_id: Uuid) -> Result<Vec<Board>, TaskError> {
        let rows = sqlx::query_as::<_, BoardRow>(
            r#"
            SELECT id, tenant_id, name, default_board, dispatch_policy, pool_size, created_at
            FROM   boards
            WHERE  tenant_id = $1
            ORDER  BY name
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        Ok(rows.into_iter().map(Board::from).collect())
    }

    async fn create_board(&self, req: CreateBoardRequest) -> Result<Board, TaskError> {
        let id = Uuid::now_v7();
        let dispatch_policy = req.dispatch_policy.unwrap_or_else(|| "fifo".into());
        let pool_size = req.pool_size.unwrap_or(5);

        let row = sqlx::query_as::<_, BoardRow>(
            r#"
            INSERT INTO boards
                (id, tenant_id, name, default_board, dispatch_policy, pool_size)
            VALUES
                ($1, $2, $3, $4, $5, $6)
            RETURNING id, tenant_id, name, default_board, dispatch_policy, pool_size, created_at
            "#,
        )
        .bind(id)
        .bind(req.tenant_id)
        .bind(&req.name)
        .bind(req.default_board)
        .bind(&dispatch_policy)
        .bind(pool_size)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("boards_tenant_name_key") => {
                TaskError::InvalidArgument(format!(
                    "board '{}' already exists for tenant",
                    req.name
                ))
            }
            _ => db_err(e),
        })?;

        Ok(Board::from(row))
    }

    // ---- Tasks ---------------------------------------------------------

    async fn list_tasks(
        &self,
        board_id: Uuid,
        column: Option<Column>,
    ) -> Result<Vec<Task>, TaskError> {
        let rows = sqlx::query_as::<_, TaskRow>(
            r#"
            SELECT id, board_id, column, title, description, priority,
                   assignee_agent, parent_task_id, blocked_reason, created_at, updated_at
            FROM   tasks
            WHERE  board_id = $1
              AND  ($2::text IS NULL OR column = $2)
            ORDER  BY priority DESC, created_at ASC
            "#,
        )
        .bind(board_id)
        .bind(column.map(|c| c.as_str().to_owned()))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(Task::try_from).collect()
    }

    async fn create_task(&self, req: CreateTaskRequest) -> Result<Task, TaskError> {
        let priority = req.priority.unwrap_or(128);
        if !(0..=255).contains(&priority) {
            return Err(TaskError::InvalidArgument("priority must be 0–255".into()));
        }
        let column = req.column.unwrap_or(Column::Triage).as_str().to_owned();
        let id = Uuid::now_v7();

        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let task_row = sqlx::query_as::<_, TaskRow>(
            r#"
            INSERT INTO tasks
                (id, board_id, column, title, description, priority,
                 assignee_agent, parent_task_id)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id, board_id, column, title, description, priority,
                      assignee_agent, parent_task_id, blocked_reason, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(req.board_id)
        .bind(&column)
        .bind(&req.title)
        .bind(&req.description)
        .bind(priority)
        .bind(&req.assignee_agent)
        .bind(req.parent_task_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        // Record creation in the log.
        sqlx::query(
            r#"
            INSERT INTO task_state_log (task_id, from_column, to_column, actor, reason)
            VALUES ($1, NULL, $2, 'system', 'created')
            "#,
        )
        .bind(id)
        .bind(&column)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        Task::try_from(task_row)
    }

    async fn update_task_column(
        &self,
        task_id: Uuid,
        new_column: Column,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<Task, TaskError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // Fetch current column before update.
        let current: Option<(String,)> =
            sqlx::query_as("SELECT column FROM tasks WHERE id = $1 FOR UPDATE")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;

        let old_col_str = current
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?
            .0;

        let new_col_str = new_column.as_str();

        // Clear blocked_reason when leaving BLOCKED column.
        let clear_blocked = old_col_str == "blocked" && new_col_str != "blocked";

        let task_row = sqlx::query_as::<_, TaskRow>(
            r#"
            UPDATE tasks
            SET    column         = $2,
                   updated_at     = now(),
                   blocked_reason = CASE WHEN $3 THEN NULL ELSE blocked_reason END
            WHERE  id = $1
            RETURNING id, board_id, column, title, description, priority,
                      assignee_agent, parent_task_id, blocked_reason, created_at, updated_at
            "#,
        )
        .bind(task_id)
        .bind(new_col_str)
        .bind(clear_blocked)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query(
            r#"
            INSERT INTO task_state_log (task_id, from_column, to_column, actor, reason)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(task_id)
        .bind(&old_col_str)
        .bind(new_col_str)
        .bind(actor)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        Task::try_from(task_row)
    }

    async fn dispatch_next_ready(
        &self,
        board_id: Uuid,
        agent_id: &str,
    ) -> Result<Option<Task>, TaskError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // SELECT ... FOR UPDATE SKIP LOCKED — safe concurrent dispatch.
        let candidate: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT id FROM tasks
            WHERE  board_id = $1 AND column = 'ready'
            ORDER  BY priority DESC, created_at ASC
            LIMIT  1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(board_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;

        let Some((task_id,)) = candidate else {
            tx.commit().await.map_err(db_err)?;
            return Ok(None);
        };

        let task_row = sqlx::query_as::<_, TaskRow>(
            r#"
            UPDATE tasks
            SET    column         = 'running',
                   assignee_agent = $2,
                   updated_at     = now()
            WHERE  id = $1
            RETURNING id, board_id, column, title, description, priority,
                      assignee_agent, parent_task_id, blocked_reason, created_at, updated_at
            "#,
        )
        .bind(task_id)
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query(
            r#"
            INSERT INTO task_state_log (task_id, from_column, to_column, actor, reason)
            VALUES ($1, 'ready', 'running', $2, 'dispatched')
            "#,
        )
        .bind(task_id)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        Task::try_from(task_row).map(Some)
    }

    async fn block_task(
        &self,
        task_id: Uuid,
        actor: &str,
        reason: &str,
    ) -> Result<Task, TaskError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let current: Option<(String,)> =
            sqlx::query_as("SELECT column FROM tasks WHERE id = $1 FOR UPDATE")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;

        let old_col_str = current
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?
            .0;

        let task_row = sqlx::query_as::<_, TaskRow>(
            r#"
            UPDATE tasks
            SET    column         = 'blocked',
                   blocked_reason = $2,
                   updated_at     = now()
            WHERE  id = $1
            RETURNING id, board_id, column, title, description, priority,
                      assignee_agent, parent_task_id, blocked_reason, created_at, updated_at
            "#,
        )
        .bind(task_id)
        .bind(reason)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;

        sqlx::query(
            r#"
            INSERT INTO task_state_log (task_id, from_column, to_column, actor, reason)
            VALUES ($1, $2, 'blocked', $3, $4)
            "#,
        )
        .bind(task_id)
        .bind(&old_col_str)
        .bind(actor)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        Task::try_from(task_row)
    }

    async fn get_task_history(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<TaskStateLogEntry>, TaskError> {
        let rows = sqlx::query_as::<_, LogRow>(
            r#"
            SELECT id, task_id, from_column, to_column, actor, reason, occurred_at
            FROM   task_state_log
            WHERE  task_id = $1
            ORDER  BY occurred_at ASC, id ASC
            "#,
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter().map(TaskStateLogEntry::try_from).collect()
    }
}
