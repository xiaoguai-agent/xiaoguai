//! Unit tests for `xiaoguai-memory`.
//!
//! All tests use `InMemoryMemoryStore` + `InMemoryEmbedder` so they:
//! - Run without external services.
//! - Produce **deterministic** cosine similarity (no randomness in embeddings).
//! - Assert on score ordering with concrete text fixtures that were chosen to
//!   produce meaningfully different vectors via the polynomial hash scheme.

pub mod crud;
pub mod recall;
pub mod tag_filter;
pub mod ttl;
