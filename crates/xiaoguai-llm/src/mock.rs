//! Deterministic in-memory backend for tests and CI.

use async_trait::async_trait;
use futures::stream;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest};

#[derive(Debug, Clone)]
enum MockMode {
    Response(String),
    Failing(LlmError),
}

#[derive(Debug, Clone)]
pub struct MockBackend {
    mode: MockMode,
}

impl MockBackend {
    pub fn with_response(response: impl Into<String>) -> Self {
        Self {
            mode: MockMode::Response(response.into()),
        }
    }

    /// Backend that fails its initial `chat_stream` call with the given error.
    /// Used to exercise the router's fallback chain.
    #[must_use]
    pub fn failing(err: LlmError) -> Self {
        Self {
            mode: MockMode::Failing(err),
        }
    }
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        match &self.mode {
            MockMode::Failing(err) => Err(err.clone()),
            MockMode::Response(full) => {
                let chunks = vec![
                    Ok(ChatChunk {
                        delta: full.clone(),
                        done: false,
                    }),
                    Ok(ChatChunk {
                        delta: String::new(),
                        done: true,
                    }),
                ];
                Ok(Box::pin(stream::iter(chunks)))
            }
        }
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}
