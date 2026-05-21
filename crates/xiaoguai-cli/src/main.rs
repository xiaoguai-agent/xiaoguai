//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xiaoguai_cli::commands::{chat, mcp, provider};
use xiaoguai_config::Settings;
use xiaoguai_storage::{
    connect,
    repositories::{
        LlmProviderRepository, McpServerRepository, PgLlmProviderRepository, PgMcpServerRepository,
    },
};

#[derive(Parser)]
#[command(name = "xiaoguai", version, about = "Xiaoguai CLI")]
struct Cli {
    /// Path to a YAML config file. Defaults to `~/.xiaoguai/config.yaml` if
    /// the file exists, otherwise an env-driven default.
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Send a one-shot prompt to the agent and print the response.
    Chat {
        /// User prompt.
        #[arg(long)]
        prompt: String,
        /// Use the deterministic mock backend (no network).
        #[arg(long, conflicts_with = "ollama_url")]
        mock: bool,
        /// Override Ollama base URL (default <http://localhost:11434>).
        #[arg(long)]
        ollama_url: Option<String>,
        /// LLM model name (default `qwen2.5-coder`).
        #[arg(long, default_value = "qwen2.5-coder")]
        model: String,
    },

    /// Administer the LLM provider registry (Postgres-backed).
    Provider {
        #[command(subcommand)]
        action: ProviderCmd,
    },

    /// Administer the MCP server registry (Postgres-backed).
    Mcp {
        #[command(subcommand)]
        action: McpCmd,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Register a new MCP server.
    Register {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "1.0.0")]
        version: String,
        /// One of: `stdio`, `sse`, `http`.
        #[arg(long)]
        transport: String,
        /// Command to spawn (required for transport=stdio).
        #[arg(long)]
        command: Option<String>,
        /// Comma-separated args to pass to the command.
        #[arg(long, value_delimiter = ',', default_value = "")]
        args: Vec<String>,
        /// Comma-separated env-var NAMES the server needs. Values are
        /// resolved at spawn time — never stored.
        #[arg(long, value_delimiter = ',', default_value = "")]
        env_keys: Vec<String>,
        /// Endpoint URL (required for transport=sse|http).
        #[arg(long)]
        endpoint: Option<String>,
        /// Tenant id for tenant-scoped servers. Omit for system-wide.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// List MCP servers (omit `--tenant` for globals only).
    List {
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Remove an MCP server by id.
    Remove {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum ProviderCmd {
    /// Register a new provider. `--api-key-env` names an env var; the key
    /// itself is never written to the database.
    Register {
        #[arg(long)]
        name: String,
        /// One of: `ollama`, `openai_compat`.
        #[arg(long)]
        kind: String,
        #[arg(long)]
        endpoint: String,
        /// Comma-separated list of supported model names.
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// Comma-separated list of models this provider should be the
        /// default for (within its scope).
        #[arg(long, value_delimiter = ',', default_value = "")]
        default_for: Vec<String>,
        #[arg(long, default_value_t = 100)]
        fallback_order: i32,
        /// Name of the env var holding the API key.
        #[arg(long)]
        api_key_env: Option<String>,
        /// Tenant id for tenant-scoped providers. Omit for system-wide.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// List providers in a given scope (omit `--tenant` for globals only).
    List {
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Remove a provider by id.
    Remove {
        #[arg(long)]
        id: String,
    },
}

fn load_settings(config: Option<&str>) -> Result<Settings> {
    match config {
        Some(path) => {
            Settings::load_from_file(path).map_err(|e| anyhow::anyhow!("load config: {e}"))
        }
        None => Settings::load_from_env().map_err(|e| anyhow::anyhow!("load env config: {e}")),
    }
}

async fn build_provider_repo(config: Option<&str>) -> Result<PgLlmProviderRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("connect to postgres")?;
    Ok(PgLlmProviderRepository::new(pool))
}

async fn build_mcp_repo(config: Option<&str>) -> Result<PgMcpServerRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("connect to postgres")?;
    Ok(PgMcpServerRepository::new(pool))
}

async fn handle_chat(
    prompt: String,
    mock: bool,
    ollama_url: Option<String>,
    model: String,
) -> Result<()> {
    let answer = chat::run(chat::ChatArgs {
        prompt,
        mock,
        ollama_url,
        model,
    })
    .await?;
    println!("{answer}");
    Ok(())
}

async fn handle_provider(config: Option<&str>, action: ProviderCmd) -> Result<()> {
    let repo = build_provider_repo(config).await?;
    let repo: &dyn LlmProviderRepository = &repo;
    match action {
        ProviderCmd::Register {
            name,
            kind,
            endpoint,
            models,
            default_for,
            fallback_order,
            api_key_env,
            tenant,
        } => {
            let p = provider::register(
                repo,
                provider::RegisterArgs {
                    name,
                    kind,
                    endpoint,
                    models,
                    default_for: default_for.into_iter().filter(|s| !s.is_empty()).collect(),
                    fallback_order,
                    api_key_env,
                    tenant,
                },
            )
            .await?;
            println!("registered {} ({})", p.id, p.name);
        }
        ProviderCmd::List { tenant } => {
            let rows = provider::list(repo, provider::ListArgs { tenant }).await?;
            print!("{}", provider::format_table(&rows));
        }
        ProviderCmd::Remove { id } => {
            provider::remove(repo, provider::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn handle_mcp(config: Option<&str>, action: McpCmd) -> Result<()> {
    let repo = build_mcp_repo(config).await?;
    let repo: &dyn McpServerRepository = &repo;
    match action {
        McpCmd::Register {
            name,
            version,
            transport,
            command,
            args,
            env_keys,
            endpoint,
            tenant,
        } => {
            let server = mcp::register(
                repo,
                mcp::RegisterArgs {
                    name,
                    version,
                    transport,
                    command,
                    args: args.into_iter().filter(|s| !s.is_empty()).collect(),
                    env_keys: env_keys.into_iter().filter(|s| !s.is_empty()).collect(),
                    endpoint,
                    tenant,
                },
            )
            .await?;
            println!(
                "registered {} ({}@{})",
                server.id, server.name, server.version
            );
        }
        McpCmd::List { tenant } => {
            let rows = mcp::list(repo, mcp::ListArgs { tenant }).await?;
            print!("{}", mcp::format_table(&rows));
        }
        McpCmd::Remove { id } => {
            mcp::remove(repo, mcp::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let cfg = cli.config.as_deref();
    match cli.command {
        Cmd::Chat {
            prompt,
            mock,
            ollama_url,
            model,
        } => handle_chat(prompt, mock, ollama_url, model).await,
        Cmd::Provider { action } => handle_provider(cfg, action).await,
        Cmd::Mcp { action } => handle_mcp(cfg, action).await,
    }
}
