//! Xiaoguai CLI — minimal v0.1 interface.

use anyhow::Result;
use clap::{Parser, Subcommand};
use xiaoguai_agent::Agent;
use xiaoguai_llm::{LlmBackend, MockBackend, OllamaBackend};

#[derive(Parser)]
#[command(name = "xiaoguai", version, about = "Xiaoguai CLI")]
struct Cli {
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
        /// LLM model name (default "qwen2.5-coder").
        #[arg(long, default_value = "qwen2.5-coder")]
        model: String,
    },
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
            let backend: Box<dyn LlmBackend> = if mock {
                Box::new(MockBackend::with_response("Hello from Xiaoguai!"))
            } else {
                let url = ollama_url.unwrap_or_else(|| "http://localhost:11434".to_string());
                Box::new(OllamaBackend::new(url))
            };
            let agent = Agent::new(backend, model);
            let answer = agent.run_once(&prompt).await?;
            println!("{answer}");
        }
    }
    Ok(())
}
