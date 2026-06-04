//! `McpServerRepository` — `SQLite`-backed registry of MCP server manifests.
//!
//! Single-owner deployment (DEC-033): all rows belong to the one owner; every
//! method runs in a plain transaction.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use xiaoguai_types::{ids::McpServerInstanceId, McpServer, McpTransport};

use crate::repositories::error::{RepoError, RepoResult};

#[async_trait]
pub trait McpServerRepository: Send + Sync {
    async fn create(&self, server: &McpServer) -> RepoResult<()>;
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>>;
    /// All registered MCP server manifests, ordered by name + version.
    async fn list(&self) -> RepoResult<Vec<McpServer>>;
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct PgMcpServerRepository {
    pool: SqlitePool,
}

impl PgMcpServerRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct McpServerRow {
    id: String,
    name: String,
    version: String,
    transport: String,
    command: Option<String>,
    args: serde_json::Value,
    env_keys: serde_json::Value,
    endpoint: Option<String>,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl McpServerRow {
    fn into_domain(self) -> RepoResult<McpServer> {
        let transport = McpTransport::parse(&self.transport).ok_or_else(|| {
            RepoError::InvalidArgument(format!("unknown mcp transport: {}", self.transport))
        })?;
        let args: Vec<String> = serde_json::from_value(self.args)?;
        let env_keys: Vec<String> = serde_json::from_value(self.env_keys)?;
        Ok(McpServer {
            id: McpServerInstanceId::from(self.id),
            name: self.name,
            version: self.version,
            transport,
            command: self.command,
            args,
            env_keys,
            endpoint: self.endpoint,
            enabled: self.enabled,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

const SELECT_COLUMNS: &str = "id, name, version, transport, command, args, \
     env_keys, endpoint, enabled, created_at, updated_at";

#[async_trait]
impl McpServerRepository for PgMcpServerRepository {
    async fn create(&self, s: &McpServer) -> RepoResult<()> {
        let args = serde_json::to_value(&s.args)?;
        let env_keys = serde_json::to_value(&s.env_keys)?;
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query(
            "INSERT INTO mcp_servers \
             (id, name, version, transport, command, args, env_keys, \
              endpoint, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(s.id.as_str())
        .bind(&s.name)
        .bind(&s.version)
        .bind(s.transport.as_str())
        .bind(s.command.as_deref())
        .bind(args)
        .bind(env_keys)
        .bind(s.endpoint.as_deref())
        .bind(s.enabled)
        .bind(s.created_at)
        .bind(s.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let row = sqlx::query_as::<_, McpServerRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM mcp_servers WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        row.map(McpServerRow::into_domain).transpose()
    }

    async fn list(&self) -> RepoResult<Vec<McpServer>> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        let rows = sqlx::query_as::<_, McpServerRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM mcp_servers \
             ORDER BY name ASC, version ASC, created_at ASC"
        ))
        .fetch_all(&mut *tx)
        .await
        .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(McpServerRow::into_domain).collect()
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        let mut tx = self.pool.begin().await.map_err(RepoError::from_sqlx)?;
        sqlx::query("DELETE FROM mcp_servers WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(RepoError::from_sqlx)?;
        tx.commit().await.map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
