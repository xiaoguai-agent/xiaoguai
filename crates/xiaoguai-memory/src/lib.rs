//! Long-term memory for xiaoguai agents.
//!
//! Inspired by Hermes-style memory architecture: agents can persist facts,
//! episodes, and preferences across sessions with semantic (vector) retrieval.
//!
//! # Backends
//!
//! * [`SqliteMemoryStore`] — production backend backed by embedded `SQLite`.
//!   Requires the `pg` Cargo feature (on by default). Embeddings are stored
//!   as a `BLOB` of little-endian `f32`; recall applies the SQL filters then
//!   computes cosine similarity in Rust (no pgvector). See [`sqlite`] for details.
//! * [`InMemoryMemoryStore`] — deterministic in-memory backend for unit tests.
//!   Uses cosine similarity on f32 vectors; embeddings are supplied by
//!   [`InMemoryEmbedder`] which produces stable, reproducible vectors.
//!
//! # Memory kinds
//!
//! | Kind | Typical content |
//! |------|-----------------|
//! | `facts` | Stable factual knowledge (user name, company, preferences) |
//! | `episodes` | Episodic session summaries (what happened in session X) |
//! | `preferences` | Explicit user preferences or soft constraints |
//!
//! # Semantic recall
//!
//! [`MemoryStore::recall_memories`] converts a natural-language query to a
//! vector via [`EmbeddingProvider`], then fetches the `top_k` most similar
//! memories using cosine distance. The result set is also written to
//! `recall_traces` for observability.
//!
//! # TTL
//!
//! Memories with `ttl_at` set expire automatically via
//! [`MemoryStore::cleanup_expired`]. Call this periodically from a scheduler
//! job or maintenance task.

#![forbid(unsafe_code)]

pub mod embedder;
pub mod error;
pub mod store;
pub mod traits;
pub mod types;

#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use embedder::{EmbeddingProvider, InMemoryEmbedder};
pub use error::{MemoryError, MemoryResult};
pub use store::InMemoryMemoryStore;
pub use traits::MemoryStore;
pub use types::{Memory, MemoryKind, RecallTrace};

#[cfg(feature = "openai")]
pub use embedder::OpenAIEmbedder;

#[cfg(feature = "ollama")]
pub use embedder::OllamaEmbedder;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteMemoryStore;

#[cfg(test)]
mod tests;
