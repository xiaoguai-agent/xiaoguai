//! Wires long-term memory ([`SqliteMemoryStore`]) into the API state.
//!
//! The embedding backend is selected for the deployment topology: an
//! air-gapped install points `OLLAMA_HOST` at a local Ollama server and gets
//! [`OllamaEmbedder`]; anything else falls back to the deterministic,
//! dependency-free [`InMemoryEmbedder`]. Both produce 384-dim vectors, matching
//! the `vector(384)` column in migration `0019_memories.sql`, so no schema
//! change is required when switching backends.

use std::sync::Arc;

use sqlx::SqlitePool;
use xiaoguai_memory::{
    EmbeddingProvider, InMemoryEmbedder, MemoryStore, OllamaEmbedder, SqliteMemoryStore,
};

/// Which embedding backend long-term memory should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedderChoice {
    /// Local, air-gapped embeddings via an Ollama server at this base URL.
    Ollama(String),
    /// Deterministic in-process embeddings (no external service).
    InMemory,
}

impl EmbedderChoice {
    /// Decide the backend from an optional `OLLAMA_HOST` value.
    ///
    /// A non-blank `OLLAMA_HOST` selects the air-gapped Ollama backend (the
    /// value is trimmed); an unset or blank value falls back to the in-process
    /// embedder.
    #[must_use]
    pub fn from_ollama_host(host: Option<&str>) -> Self {
        match host {
            Some(h) if !h.trim().is_empty() => Self::Ollama(h.trim().to_string()),
            _ => Self::InMemory,
        }
    }

    /// Construct the corresponding embedding provider.
    fn into_provider(self) -> Arc<dyn EmbeddingProvider> {
        match self {
            Self::Ollama(host) => Arc::new(OllamaEmbedder::from_host(host)),
            Self::InMemory => Arc::new(InMemoryEmbedder::default_dim()),
        }
    }
}

/// Build the long-term memory store, selecting the embedder via `OLLAMA_HOST`.
///
/// Returned as a trait object so `AppState.memory_store` stays backend-agnostic;
/// flipping `/v1/memories` from 503 to live.
#[must_use]
pub fn build_memory_store(pool: SqlitePool) -> Arc<dyn MemoryStore> {
    let host = std::env::var("OLLAMA_HOST").ok();
    let choice = EmbedderChoice::from_ollama_host(host.as_deref());
    tracing::info!(?choice, "memory: selected embedding backend");
    Arc::new(SqliteMemoryStore::new(pool, choice.into_provider()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_host_falls_back_to_in_memory() {
        assert_eq!(
            EmbedderChoice::from_ollama_host(None),
            EmbedderChoice::InMemory
        );
    }

    #[test]
    fn blank_host_falls_back_to_in_memory() {
        assert_eq!(
            EmbedderChoice::from_ollama_host(Some("   ")),
            EmbedderChoice::InMemory
        );
    }

    #[test]
    fn nonblank_host_selects_ollama_and_trims() {
        assert_eq!(
            EmbedderChoice::from_ollama_host(Some("  http://localhost:11434  ")),
            EmbedderChoice::Ollama("http://localhost:11434".to_string())
        );
    }
}
