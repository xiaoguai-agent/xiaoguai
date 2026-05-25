//! `xiaoguai chat` — one-shot prompt against a single backend.

use anyhow::Result;
use xiaoguai_agent::Agent;
use xiaoguai_llm::{LlmBackend, MockBackend, OllamaBackend};

#[derive(Debug, Clone)]
pub struct ChatArgs {
    pub prompt: String,
    pub mock: bool,
    pub ollama_url: Option<String>,
    pub model: String,
}

/// Execute a single `chat` invocation and return the assistant reply.
///
/// Kept testable: the binary entry point in `main.rs` just delegates here.
///
/// # Errors
/// Returns an error if the LLM backend cannot be constructed or the API call fails.
pub async fn run(args: ChatArgs) -> Result<String> {
    let backend: Box<dyn LlmBackend> = if args.mock {
        Box::new(MockBackend::with_response("Hello from Xiaoguai!"))
    } else {
        let url = args
            .ollama_url
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        Box::new(OllamaBackend::new(url))
    };
    let agent = Agent::new(backend, args.model);
    agent.run_once(&args.prompt).await
}
