//! `LlmBackend` trait — object-safe abstraction over LLM providers.

use std::pin::Pin;

use async_trait::async_trait;

use crate::types::{ChatChunk, ChatRequest};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("network: {0}")]
    Network(String),
    #[error("provider returned error: {0}")]
    Provider(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

pub type ChatStream = Pin<Box<dyn futures::Stream<Item = Result<ChatChunk, LlmError>> + Send>>;

#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Stream a chat completion. Returns chunks until `done: true`.
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError>;

    /// Backend identifier used in logs and metrics, e.g. `"ollama"`, `"mock"`.
    fn name(&self) -> &'static str;
}

// Compile-time check that the trait stays object-safe.
const _: Option<Box<dyn LlmBackend>> = None;
