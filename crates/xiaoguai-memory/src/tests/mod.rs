//! Unit tests for `xiaoguai-memory`.
//!
//! All tests use `InMemoryMemoryStore` + `InMemoryEmbedder` so they:
//! - Run without Postgres (no pgvector required).
//! - Produce **deterministic** cosine similarity (no randomness in embeddings).
//! - Assert on score ordering with concrete text fixtures that were chosen to
//!   produce meaningfully different vectors via the polynomial hash scheme.

pub mod crud;
pub mod multi_tenant;
pub mod recall;
pub mod tag_filter;
pub mod ttl;
