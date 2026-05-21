//! Deterministic in-memory backend for tests and CI.

use async_trait::async_trait;
use futures::stream;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest};

#[derive(Debug, Clone)]
pub struct MockBackend {
    response: String,
}

impl MockBackend {
    pub fn with_response(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

#[async_trait]
impl LlmBackend for MockBackend {
    async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        let full = self.response.clone();
        let chunks = vec![
            Ok(ChatChunk {
                delta: full,
                done: false,
            }),
            Ok(ChatChunk {
                delta: String::new(),
                done: true,
            }),
        ];
        Ok(Box::pin(stream::iter(chunks)))
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}
