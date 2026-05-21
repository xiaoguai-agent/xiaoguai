//! `McpServerRepository` — Postgres-backed registry of MCP server manifests.
//!
//! Pattern mirrors `llm_provider.rs`: `tenant_id IS NULL` = system-wide, RLS
//! policy `tenant_or_global_isolation_mcp` enforces visibility at the DB
//! layer. Tests connect as superuser and exercise scope filtering via the
//! repo's own WHERE clauses.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use xiaoguai_types::{
    ids::{McpServerInstanceId, TenantId},
    McpServer, McpTransport,
};

use crate::repositories::error::{RepoError, RepoResult};

#[async_trait]
pub trait McpServerRepository: Send + Sync {
    async fn create(&self, server: &McpServer) -> RepoResult<()>;
    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>>;
    /// System-wide rows (tenant_id IS NULL), ordered by name + version.
    async fn list_global(&self) -> RepoResult<Vec<McpServer>>;
    /// System-wide rows plus rows scoped to `tenant_id`.
    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<McpServer>>;
    async fn delete(&self, id: &str) -> RepoResult<()>;
}

#[derive(Debug, Clone)]
pub struct PgMcpServerRepository {
    pool: PgPool,
}

impl PgMcpServerRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct McpServerRow {
    id: String,
    tenant_id: Option<String>,
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
            tenant_id: self.tenant_id.map(TenantId::from),
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

const SELECT_COLUMNS: &str = "id, tenant_id, name, version, transport, command, args, \
     env_keys, endpoint, enabled, created_at, updated_at";

#[async_trait]
impl McpServerRepository for PgMcpServerRepository {
    async fn create(&self, s: &McpServer) -> RepoResult<()> {
        let args = serde_json::to_value(&s.args)?;
        let env_keys = serde_json::to_value(&s.env_keys)?;
        sqlx::query(
            "INSERT INTO mcp_servers \
             (id, tenant_id, name, version, transport, command, args, env_keys, \
              endpoint, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(s.id.as_str())
        .bind(s.tenant_id.as_ref().map(AsRef::as_ref))
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
        .execute(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<McpServer>> {
        let row = sqlx::query_as::<_, McpServerRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM mcp_servers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        row.map(McpServerRow::into_domain).transpose()
    }

    async fn list_global(&self) -> RepoResult<Vec<McpServer>> {
        let rows = sqlx::query_as::<_, McpServerRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM mcp_servers \
             WHERE tenant_id IS NULL \
             ORDER BY name ASC, version ASC, created_at ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(McpServerRow::into_domain).collect()
    }

    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<McpServer>> {
        let rows = sqlx::query_as::<_, McpServerRow>(&format!(
            "SELECT {SELECT_COLUMNS} FROM mcp_servers \
             WHERE tenant_id IS NULL OR tenant_id = $1 \
             ORDER BY (tenant_id IS NOT NULL) ASC, name ASC, version ASC, created_at ASC"
        ))
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepoError::from_sqlx)?;
        rows.into_iter().map(McpServerRow::into_domain).collect()
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        sqlx::query("DELETE FROM mcp_servers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(RepoError::from_sqlx)?;
        Ok(())
    }
}
