//! LLM router and backends.
//!
//! v0.1 ships:
//!   - `LlmBackend` trait (object-safe)
//!   - `MockBackend` for tests / CI
//!   - `OllamaBackend` for local development
//!
//! v0.5 adds: OpenAI-compatible backend, vLLM backend, provider routing.

pub mod backend;
pub mod breaker;
pub mod mock;
pub mod ollama;
pub mod openai_compat;
pub mod router;
pub mod types;
pub mod usage;

pub use backend::{ChatStream, LlmBackend, LlmError};
pub use breaker::{Breaker, BreakerConfig, BreakerState, Breakers, Clock, SystemClock};
pub use mock::MockBackend;
pub use ollama::OllamaBackend;
pub use openai_compat::OpenAiCompatBackend;
pub use router::{LlmRouter, ResolveCtx, RouterConfig};
pub use types::{ChatChunk, ChatRequest, Message, Role};
pub use usage::{BufferedUsageSink, MemoryUsageSink, UsageRecord, UsageSink};
