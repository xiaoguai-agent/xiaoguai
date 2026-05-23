//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xiaoguai_cli::commands::{chat, eval, mcp, provider, remote};
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

    /// Talk to a running `xiaoguai-api` over HTTP/SSE.
    Remote {
        /// Base URL of the API server, e.g. `http://localhost:8080`.
        #[arg(long)]
        server: String,
        #[command(subcommand)]
        action: RemoteCmd,
    },

    /// Run an eval suite (`*.eval.yaml` cases) against the deterministic
    /// `MockBackend` substrate and print pass/fail.
    Eval {
        #[command(subcommand)]
        action: EvalCmd,
    },
}

#[derive(Subcommand)]
enum EvalCmd {
    /// Walk a directory of `.eval.yaml` cases and grade them.
    Run {
        /// Suite name. Becomes `EvalReport.suite`; also the default
        /// case directory under `./eval/<suite>` when `--cases-dir`
        /// is omitted.
        #[arg(long)]
        suite: String,
        /// Directory holding `*.eval.yaml` files. Flat (no recursion).
        #[arg(long)]
        cases_dir: Option<String>,
        /// Optional path to write the JSON report to. Omit for
        /// stdout-only output.
        #[arg(long)]
        out: Option<String>,
        /// Override the agent loop's `max_iterations`. `0` = use
        /// `AgentConfig::new`'s default (8).
        #[arg(long, default_value_t = 0)]
        max_iterations: u32,
    },
}

#[derive(Subcommand)]
enum RemoteCmd {
    /// Smoke test the remote server.
    Healthz,
    /// Send one prompt against a fresh session and print the streamed reply.
    Chat {
        #[arg(long)]
        user_id: String,
        #[arg(long)]
        tenant_id: String,
        #[arg(long, default_value = "qwen2.5-coder")]
        model: String,
        #[arg(long)]
        prompt: String,
        /// Optional title for the new session.
        #[arg(long)]
        title: Option<String>,
    },
    /// Fetch and print the message history of an existing session.
    Messages {
        #[arg(long)]
        session: String,
    },
    /// Cancel an in-flight agent run.
    Cancel {
        #[arg(long)]
        session: String,
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

async fn handle_remote(server: String, action: RemoteCmd) -> Result<()> {
    use std::io::Write;
    let client = remote::RemoteClient::new(server);
    match action {
        RemoteCmd::Healthz => {
            let body = client.healthz().await?;
            println!("{body}");
        }
        RemoteCmd::Chat {
            user_id,
            tenant_id,
            model,
            prompt,
            title,
        } => {
            let session = client
                .create_session(&remote::CreateSessionRequest {
                    user_id,
                    tenant_id,
                    model,
                    title,
                })
                .await?;
            eprintln!("session: {}", session.id);
            client
                .send_message(&session.id, &prompt, |ev| {
                    match ev.name.as_str() {
                        "text_delta" => {
                            if let Some(delta) =
                                ev.payload.get("delta").and_then(serde_json::Value::as_str)
                            {
                                print!("{delta}");
                                std::io::stdout().flush().ok();
                            }
                        }
                        "tool_call_started" => {
                            let name = ev
                                .payload
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("\n[tool start] {name}");
                        }
                        "tool_call_finished" => {
                            let ok = ev
                                .payload
                                .get("ok")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            eprintln!("[tool finish] ok={ok}");
                        }
                        "done" => {
                            println!();
                            let reason = ev
                                .payload
                                .get("stop_reason")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("[done] {reason}");
                        }
                        "error" => {
                            let msg = ev
                                .payload
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("[error] {msg}");
                        }
                        _ => {}
                    }
                    Ok(())
                })
                .await?;
        }
        RemoteCmd::Messages { session } => {
            let msgs = client.list_messages(&session).await?;
            println!("{}", serde_json::to_string_pretty(&msgs)?);
        }
        RemoteCmd::Cancel { session } => {
            let cancelled = client.cancel(&session).await?;
            println!("cancelled={cancelled}");
        }
    }
    Ok(())
}

async fn handle_eval(action: EvalCmd) -> Result<()> {
    match action {
        EvalCmd::Run {
            suite,
            cases_dir,
            out,
            max_iterations,
        } => {
            let report = eval::run(eval::EvalArgs {
                suite,
                cases_dir: cases_dir.map(std::path::PathBuf::from),
                out: out.map(std::path::PathBuf::from),
                max_iterations,
            })
            .await?;
            print!("{}", xiaoguai_eval::pretty_summary(&report));
            if report.failed() > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
    }
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
        Cmd::Remote { server, action } => handle_remote(server, action).await,
        Cmd::Eval { action } => handle_eval(action).await,
    }
}
