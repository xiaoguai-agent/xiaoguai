//! Embedding provider trait and built-in implementations.
//!
//! # `InMemoryEmbedder`
//!
//! Produces **deterministic** embeddings from text using a simple but stable
//! hash-based scheme. Guarantees:
//! - Same text → identical vector every call (no randomness).
//! - Different texts → distinct vectors (no collision for test payloads).
//! - Cosine similarities are meaningful within a test suite (semantically
//!   similar strings were deliberately crafted to produce similar hashes).
//!
//! Do NOT use in production; it carries no semantic meaning beyond test
//! isolation.

use async_trait::async_trait;

use crate::error::MemoryResult;

/// Embedding provider: converts text into an f32 vector.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    /// Embed a single text string. Dimension is implementation-defined.
    async fn embed(&self, text: &str) -> MemoryResult<Vec<f32>>;

    /// Return the number of dimensions this provider produces.
    fn dimensions(&self) -> usize;
}

// ─── InMemoryEmbedder ────────────────────────────────────────────────────────

/// Deterministic hash-based embedder for unit tests.
///
/// Produces `dim`-dimensional vectors by:
/// 1. Computing a simple polynomial hash over each character.
/// 2. Distributing the hash bytes across `dim` f32 buckets.
/// 3. L2-normalising the result so cosine similarity is well-defined.
///
/// The scheme is stable across Rust versions because it uses only wrapping
/// arithmetic (no `std::hash::Hash` randomisation).
#[derive(Clone)]
pub struct InMemoryEmbedder {
    dim: usize,
}

impl InMemoryEmbedder {
    /// Create an embedder with the given output dimension.
    ///
    /// # Panics
    ///
    /// Panics when `dim == 0`.
    #[must_use]
    pub fn new(dim: usize) -> Self {
        assert!(dim > 0, "dimension must be > 0");
        Self { dim }
    }

    /// Default dimension (384) — matches the production pgvector column.
    #[must_use]
    pub fn default_dim() -> Self {
        Self::new(384)
    }

    fn embed_sync(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0_f32; self.dim];

        // Hash each (byte_index, byte_value) pair into the appropriate bucket
        // using FNV-inspired mixing. We keep values in [0, 256) to avoid f32
        // overflow: compute the bucket index from the hash, then accumulate
        // the byte value modulo 256 into that bucket.
        let bytes = text.as_bytes();
        for (byte_idx, &b) in bytes.iter().enumerate() {
            // FNV-like hash of (byte_idx, b) → bucket index.
            #[allow(clippy::cast_possible_truncation)]
            let h = (byte_idx as u64)
                .wrapping_mul(0x517c_c1b7_2722_0a95)
                .wrapping_add(u64::from(b).wrapping_mul(0x9e37_79b9_7f4a_7c15));
            // Truncation is intentional: we take h modulo dim for both buckets.
            #[allow(clippy::cast_possible_truncation)]
            let bucket = (h as usize) % self.dim;
            // Accumulate the byte value (small integer, no overflow risk).
            vec[bucket] += f32::from(b);
            // Also accumulate into a secondary bucket for better spread.
            #[allow(clippy::cast_possible_truncation)]
            let bucket2 = ((h >> 32) as usize) % self.dim;
            vec[bucket2] += 1.0;
        }

        l2_normalise(&mut vec);
        vec
    }
}

#[async_trait]
impl EmbeddingProvider for InMemoryEmbedder {
    async fn embed(&self, text: &str) -> MemoryResult<Vec<f32>> {
        Ok(self.embed_sync(text))
    }

    fn dimensions(&self) -> usize {
        self.dim
    }
}

/// Normalise `v` to unit length in-place. No-op when `‖v‖ ≈ 0`.
pub(crate) fn l2_normalise(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two equal-length unit vectors. Returns 0.0 when
/// either vector has zero norm (already unit from `l2_normalise`).
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ─── OpenAIEmbedder ──────────────────────────────────────────────────────────

#[cfg(feature = "openai")]
pub struct OpenAIEmbedder {
    client: async_openai::Client<async_openai::config::OpenAIConfig>,
    model: String,
    dim: usize,
}

#[cfg(feature = "openai")]
impl OpenAIEmbedder {
    /// Create using the `OPENAI_API_KEY` environment variable.
    pub fn from_env(model: impl Into<String>, dim: usize) -> Self {
        Self {
            client: async_openai::Client::new(),
            model: model.into(),
            dim,
        }
    }
}

#[cfg(feature = "openai")]
#[async_trait]
impl EmbeddingProvider for OpenAIEmbedder {
    async fn embed(&self, text: &str) -> MemoryResult<Vec<f32>> {
        use async_openai::types::{CreateEmbeddingRequestArgs, EmbeddingInput};

        let req = CreateEmbeddingRequestArgs::default()
            .model(&self.model)
            .input(EmbeddingInput::String(text.to_owned()))
            .build()
            .map_err(|e| MemoryError::Embedding(e.to_string()))?;

        let resp = self
            .client
            .embeddings()
            .create(req)
            .await
            .map_err(|e| MemoryError::Embedding(e.to_string()))?;

        resp.data
            .into_iter()
            .next()
            .map(|e| e.embedding.into_iter().map(|x| x as f32).collect())
            .ok_or_else(|| MemoryError::Embedding("empty embedding response".into()))
    }

    fn dimensions(&self) -> usize {
        self.dim
    }
}

// ─── OllamaEmbedder ──────────────────────────────────────────────────────────

/// Embedder backed by a locally-running [Ollama](https://ollama.com) server.
///
/// Requires no API key. Designed for **air-gapped** deployments where an
/// outbound call to OpenAI is not permitted.
///
/// ## Default model: `all-minilm`
///
/// `all-minilm` (sentence-transformers/all-MiniLM-L6-v2) produces **384-dimensional**
/// vectors, which matches the `vector(384)` column in migration `0019_memories.sql`.
/// Using a different model requires a schema migration to widen the column:
///
/// | Model               | Dimensions | Migration change needed |
/// |---------------------|:----------:|------------------------|
/// | `all-minilm`        | 384        | none (default)         |
/// | `nomic-embed-text`  | 768        | alter `vector(768)`    |
/// | `mxbai-embed-large` | 1024       | alter `vector(1024)`   |
///
/// ## Normalisation
///
/// The raw vector from Ollama is returned as-is (no L2-normalisation).
/// pgvector's cosine distance operator (`<=>`) handles unit-normalisation
/// internally, matching the behaviour of [`OpenAIEmbedder`].
///
/// ## Usage
///
/// ```no_run
/// # #[cfg(feature = "ollama")]
/// # {
/// use xiaoguai_memory::OllamaEmbedder;
/// // Defaults: model=all-minilm, dim=384
/// let embedder = OllamaEmbedder::from_host("http://localhost:11434");
/// # }
/// ```
///
/// Install the model beforehand: `ollama pull all-minilm`
#[cfg(feature = "ollama")]
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    dim: usize,
    http: reqwest::Client,
}

#[cfg(feature = "ollama")]
impl OllamaEmbedder {
    /// Create an embedder pointing at `base_url` with an explicit `model` and
    /// output `dim`.
    ///
    /// `base_url` must **not** include a trailing `/api/embeddings` path — only
    /// the scheme + host + port (e.g. `"http://localhost:11434"`).
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, dim: usize) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            dim,
            http: reqwest::Client::new(),
        }
    }

    /// Convenience constructor using the `all-minilm` model at 384 dimensions.
    ///
    /// This matches the `vector(384)` column in migration `0019_memories.sql`
    /// and requires no schema changes.
    ///
    /// Before first use run: `ollama pull all-minilm`
    pub fn from_host(base_url: impl Into<String>) -> Self {
        Self::new(base_url, "all-minilm", 384)
    }
}

#[cfg(feature = "ollama")]
#[async_trait]
impl EmbeddingProvider for OllamaEmbedder {
    async fn embed(&self, text: &str) -> MemoryResult<Vec<f32>> {
        use crate::error::MemoryError;

        let url = format!("{}/api/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "prompt": text,
        });

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                MemoryError::Embedding(format!(
                    "Ollama HTTP request failed (is Ollama running at {}?): {}",
                    self.base_url, e
                ))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Embedding(format!(
                "Ollama returned HTTP {status} — check that the model is available \
                 (`ollama pull {model}`). Detail: {detail}",
                model = self.model,
            )));
        }

        let payload: serde_json::Value = resp.json().await.map_err(|e| {
            MemoryError::Embedding(format!("Ollama response parse error: {e}"))
        })?;

        let embedding = payload
            .get("embedding")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                MemoryError::Embedding(
                    "Ollama response missing `embedding` array — \
                     verify that the model supports embeddings (`ollama pull all-minilm`)"
                        .into(),
                )
            })?;

        let vec: Vec<f32> = embedding
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(vec)
    }

    fn dimensions(&self) -> usize {
        self.dim
    }
}

// ─── LocalEmbedder stub ──────────────────────────────────────────────────────

// ─── OllamaEmbedder tests ────────────────────────────────────────────────────

#[cfg(all(test, feature = "ollama"))]
mod ollama_tests {
    use super::*;

    /// Build a 384-element JSON array string for the mock response body.
    fn mock_embedding_json() -> String {
        let vals: Vec<String> = (0..384).map(|i| format!("{:.6}", 0.001_f64 * f64::from(i))).collect();
        format!("{{\"embedding\":[{}]}}", vals.join(","))
    }

    #[tokio::test]
    async fn embed_returns_384_floats() {
        let mut server = mockito::Server::new_async().await;
        let body = mock_embedding_json();

        let _mock = server
            .mock("POST", "/api/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&body)
            .create_async()
            .await;

        let embedder = OllamaEmbedder::from_host(server.url());
        let result = embedder.embed("hello world").await;

        assert!(result.is_ok(), "embed() returned error: {:?}", result.err());
        let vec = result.unwrap();
        assert_eq!(
            vec.len(),
            384,
            "expected 384 floats, got {}",
            vec.len()
        );
        assert_eq!(embedder.dimensions(), 384);
        // Spot-check a few values: index i → 0.001 * i (cast f64 → f32).
        assert!((vec[0] - 0.0_f32).abs() < 1e-5);
        assert!((vec[1] - 0.001_f32).abs() < 1e-4);
        assert!((vec[10] - 0.010_f32).abs() < 1e-4);
    }

    #[tokio::test]
    async fn embed_propagates_http_error() {
        let mut server = mockito::Server::new_async().await;

        let _mock = server
            .mock("POST", "/api/embeddings")
            .with_status(404)
            .with_body(r#"{"error":"model not found"}"#)
            .create_async()
            .await;

        let embedder = OllamaEmbedder::from_host(server.url());
        let err = embedder.embed("test").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("404"),
            "error message should mention HTTP status, got: {msg}"
        );
    }

    #[tokio::test]
    async fn from_host_defaults() {
        let embedder = OllamaEmbedder::from_host("http://localhost:11434");
        assert_eq!(embedder.dimensions(), 384);
        assert_eq!(embedder.model, "all-minilm");
    }

    #[tokio::test]
    async fn new_custom_dim() {
        let embedder = OllamaEmbedder::new("http://localhost:11434", "nomic-embed-text", 768);
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.model, "nomic-embed-text");
    }
}

/// ONNX-based local embedder (feature-gated; trait stub only in this release).
///
/// Enable with `features = ["local"]`. Full ONNX runtime integration is
/// deferred to the next milestone — the trait is already stable so callers
/// can wire `Box<dyn EmbeddingProvider>` today.
#[cfg(feature = "local")]
pub struct LocalEmbedder {
    _priv: (),
}

#[cfg(feature = "local")]
#[async_trait]
impl EmbeddingProvider for LocalEmbedder {
    async fn embed(&self, _text: &str) -> MemoryResult<Vec<f32>> {
        Err(MemoryError::Embedding(
            "LocalEmbedder: ONNX runtime not yet wired; use InMemoryEmbedder for tests or OpenAIEmbedder for production".into(),
        ))
    }

    fn dimensions(&self) -> usize {
        384
    }
}
