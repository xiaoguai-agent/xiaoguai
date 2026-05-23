//! `xiaoguai mcp {register,list,remove}` — administer the MCP server
//! registry stored in Postgres.
//!
//! Mirrors `commands::provider` exactly: pure functions taking a
//! `&dyn McpServerRepository`, so unit tests can swap in an in-memory
//! implementation.
//!
//! Secrets policy: registrations accept `--env-keys FOO,BAR` (env-variable
//! NAMES only). Values are resolved by the spawning code at supervisor
//! start time — never persisted in the database or shell history.

use anyhow::{anyhow, Result};
use chrono::Utc;
use xiaoguai_storage::repositories::McpServerRepository;
use xiaoguai_types::{
    ids::{McpServerInstanceId, TenantId},
    McpServer, McpTransport,
};

#[derive(Debug, Clone)]
pub struct RegisterArgs {
    pub name: String,
    pub version: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub endpoint: Option<String>,
    pub tenant: Option<String>,
}

pub async fn register(repo: &dyn McpServerRepository, args: RegisterArgs) -> Result<McpServer> {
    if args.name.trim().is_empty() {
        return Err(anyhow!("--name must not be empty"));
    }
    if args.version.trim().is_empty() {
        return Err(anyhow!("--version must not be empty"));
    }
    let transport = McpTransport::parse(&args.transport).ok_or_else(|| {
        anyhow!(
            "unknown transport '{}': expected 'stdio', 'sse', or 'http'",
            args.transport
        )
    })?;
    match transport {
        McpTransport::Stdio => {
            if args.command.as_deref().is_none_or(str::is_empty) {
                return Err(anyhow!("--command is required for transport=stdio"));
            }
        }
        McpTransport::Sse | McpTransport::Http => {
            if args.endpoint.as_deref().is_none_or(str::is_empty) {
                return Err(anyhow!(
                    "--endpoint is required for transport={}",
                    transport.as_str()
                ));
            }
        }
    }

    let now = Utc::now();
    let tenant_guc = args.tenant.clone();
    let server = McpServer {
        id: McpServerInstanceId::new(),
        tenant_id: args.tenant.map(TenantId::from),
        name: args.name,
        version: args.version,
        transport,
        command: args.command,
        args: args.args,
        env_keys: args.env_keys,
        endpoint: args.endpoint,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    repo.create(tenant_guc.as_deref(), &server).await?;
    Ok(server)
}

#[derive(Debug, Clone, Default)]
pub struct ListArgs {
    pub tenant: Option<String>,
}

pub async fn list(repo: &dyn McpServerRepository, args: ListArgs) -> Result<Vec<McpServer>> {
    let rows = match args.tenant {
        Some(t) => repo.list_for_tenant(&t).await?,
        None => repo.list_global().await?,
    };
    Ok(rows)
}

#[derive(Debug, Clone)]
pub struct RemoveArgs {
    pub id: String,
}

pub async fn remove(repo: &dyn McpServerRepository, args: RemoveArgs) -> Result<()> {
    if args.id.trim().is_empty() {
        return Err(anyhow!("--id must not be empty"));
    }
    // Admin CLI: caller may not know the tenant; rely on superuser/owner
    // bypass for RLS. v0.6.2 should add a `--tenant` flag to scope deletes.
    repo.delete(None, &args.id).await?;
    Ok(())
}

#[must_use]
pub fn format_table(rows: &[McpServer]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "ID                                     SCOPE       TRANSPORT  NAME             VERSION  ENABLED\n",
    );
    for s in rows {
        let scope = s
            .tenant_id
            .as_ref()
            .map_or_else(|| "global".to_string(), |t| t.as_str().to_string());
        let _ = writeln!(
            out,
            "{:38} {:11} {:10} {:16} {:8} {}",
            s.id.as_str(),
            scope,
            s.transport.as_str(),
            s.name,
            s.version,
            s.enabled
        );
    }
    out
}
