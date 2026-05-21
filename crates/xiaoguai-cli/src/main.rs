//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xiaoguai_cli::commands::{chat, provider};
use xiaoguai_config::Settings;
use xiaoguai_storage::{
    connect,
    repositories::{LlmProviderRepository, PgLlmProviderRepository},
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

async fn build_provider_repo(config: Option<&str>) -> Result<PgLlmProviderRepository> {
    let settings = match config {
        Some(path) => {
            Settings::load_from_file(path).map_err(|e| anyhow::anyhow!("load config: {e}"))?
        }
        None => Settings::load_from_env().map_err(|e| anyhow::anyhow!("load env config: {e}"))?,
    };
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("connect to postgres")?;
    Ok(PgLlmProviderRepository::new(pool))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Cmd::Chat {
            prompt,
            mock,
            ollama_url,
            model,
        } => {
            let answer = chat::run(chat::ChatArgs {
                prompt,
                mock,
                ollama_url,
                model,
            })
            .await?;
            println!("{answer}");
        }

        Cmd::Provider { action } => {
            let repo = build_provider_repo(cli.config.as_deref()).await?;
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
                            default_for: default_for
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect(),
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
        }
    }
    Ok(())
}
