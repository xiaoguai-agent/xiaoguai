//! Single-turn LLM-call agent. The full ReAct loop lands in v0.5.

use anyhow::{Context, Result};
use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, LlmBackend, Message};

pub struct Agent {
    backend: Box<dyn LlmBackend>,
    model: String,
}

impl Agent {
    pub fn new(backend: Box<dyn LlmBackend>, model: impl Into<String>) -> Self {
        Self {
            backend,
            model: model.into(),
        }
    }

    /// One-shot: send the user's prompt, return the assistant's full reply.
    ///
    /// Preserved for v0.1 callers and smoke tests. The real ReAct loop lives
    /// in [`crate::react`].
    ///
    /// # Errors
    /// Returns an error if the backend fails to stream a response.
    pub async fn run_once(&self, user_prompt: &str) -> Result<String> {
        let mut req = ChatRequest::new(self.model.clone(), vec![Message::user(user_prompt)]);
        req.temperature = Some(0.2);

        let mut stream = self
            .backend
            .chat_stream(req)
            .await
            .context("backend chat_stream failed")?;

        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("stream chunk error")?;
            out.push_str(&chunk.delta);
        }
        Ok(out)
    }
}
