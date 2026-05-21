//! LLM router and backends.
//!
//! v0.1 ships:
//!   - `LlmBackend` trait (object-safe)
//!   - `MockBackend` for tests / CI
//!   - `OllamaBackend` for local development
//!
//! v0.5 adds: OpenAI-compatible backend, vLLM backend, provider routing.

pub mod backend;
pub mod mock;
pub mod ollama;
pub mod types;

pub use backend::{ChatStream, LlmBackend, LlmError};
pub use mock::MockBackend;
pub use ollama::OllamaBackend;
pub use types::{ChatChunk, ChatRequest, Message, Role};
