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
use xiaoguai_config::EmbedderSettings;
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

    /// Decide the backend from the `memory.embedder` config block (DEC-036).
    #[must_use]
    pub fn from_settings(embedder: &EmbedderSettings) -> Self {
        match embedder {
            EmbedderSettings::Ollama { host } if !host.trim().is_empty() => {
                Self::Ollama(host.trim().to_string())
            }
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

/// Build the long-term memory store, selecting the embedder from the
/// `memory.embedder` config block (DEC-036), with the `OLLAMA_HOST` env as a
/// back-compat **override** (a non-blank value wins, so existing air-gapped
/// installs keep working without touching their config).
///
/// Returned as a trait object so `AppState.memory_store` stays backend-agnostic;
/// flipping `/v1/memories` from 503 to live.
#[must_use]
pub fn build_memory_store(pool: SqlitePool, embedder: &EmbedderSettings) -> Arc<dyn MemoryStore> {
    // `OLLAMA_HOST` env wins when it names a backend; otherwise use the config.
    let choice =
        match EmbedderChoice::from_ollama_host(std::env::var("OLLAMA_HOST").ok().as_deref()) {
            env_ollama @ EmbedderChoice::Ollama(_) => env_ollama,
            EmbedderChoice::InMemory => EmbedderChoice::from_settings(embedder),
        };
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

    #[test]
    fn config_ollama_selects_ollama() {
        assert_eq!(
            EmbedderChoice::from_settings(&EmbedderSettings::Ollama {
                host: "  http://localhost:11434  ".to_string(),
            }),
            EmbedderChoice::Ollama("http://localhost:11434".to_string())
        );
    }

    #[test]
    fn config_in_memory_and_blank_ollama_fall_back() {
        assert_eq!(
            EmbedderChoice::from_settings(&EmbedderSettings::InMemory),
            EmbedderChoice::InMemory
        );
        assert_eq!(
            EmbedderChoice::from_settings(&EmbedderSettings::Ollama {
                host: "   ".to_string()
            }),
            EmbedderChoice::InMemory
        );
    }
}
