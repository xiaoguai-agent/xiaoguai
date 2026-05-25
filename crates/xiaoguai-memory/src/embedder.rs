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

// ─── LocalEmbedder stub ──────────────────────────────────────────────────────

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
