//! LLM router and backends.
//!
//! v0.1 ships:
//!   - `LlmBackend` trait (object-safe)
//!   - `MockBackend` for tests / CI
//!   - `OllamaBackend` for local development
//!
//! v0.5 adds: OpenAI-compatible backend, vLLM backend, provider routing.
//!
//! v1.1.9 adds: `AnthropicBackend` (Messages API) and `GeminiBackend`
//! (generateContent API) for native cloud-LLM support.
//!
//! v1.2.6 adds: `BedrockBackend` (AWS Bedrock, `SigV4` hand-rolled),
//! `AzureOpenAiBackend` (Azure OpenAI, `api-key` auth), `MistralBackend`
//! (Mistral La Plateforme), and `GroqBackend` (Groq fast inference).
//!
//! **OpenAI-compatible providers** (通义/DeepSeek/智谱) already work via
//! `OpenAiCompatBackend` with their respective base URLs — they are NOT
//! duplicated here.

pub mod anthropic;
pub mod azure_openai;
pub mod backend;
pub mod bedrock;
pub mod breaker;
pub mod build;
pub mod gemini;
pub mod groq;
pub mod minimax;
pub mod mistral;
pub mod mock;
pub mod ollama;
pub mod openai_compat;
pub mod router;
pub mod token_count;
pub mod types;
pub mod usage;

pub use anthropic::AnthropicBackend;
pub use azure_openai::AzureOpenAiBackend;
pub use backend::{ChatStream, LlmBackend, LlmError};
pub use bedrock::BedrockBackend;
pub use breaker::{Breaker, BreakerConfig, BreakerState, Breakers, Clock, SystemClock};
pub use build::{build_router, BuildReport, EnvResolver, OsEnvResolver};
pub use gemini::GeminiBackend;
pub use groq::GroqBackend;
pub use minimax::MinimaxBackend;
pub use mistral::MistralBackend;
pub use mock::MockBackend;
pub use ollama::OllamaBackend;
pub use openai_compat::OpenAiCompatBackend;
pub use router::{LlmRouter, ResolveCtx, RouterConfig};
pub use token_count::{estimate_message_tokens, estimate_tokens};
pub use types::{
    ChatChunk, ChatRequest, FinishReason, Message, Role, ToolCallSpec, ToolChoice, ToolSpec,
};
pub use usage::{BufferedUsageSink, MemoryUsageSink, UsageRecord, UsageSink};
