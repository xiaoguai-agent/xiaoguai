//! `RagClient` — backend-agnostic RAG operations.

use async_trait::async_trait;
use thiserror::Error;

use crate::types::{Collection, IngestRequest, IngestResult, SearchRequest, SearchResult};

#[derive(Debug, Error)]
pub enum RagError {
    #[error("backend: {0}")]
    Backend(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

pub type RagResult<T> = Result<T, RagError>;

#[async_trait]
pub trait RagClient: Send + Sync {
    /// List known collections. Roughly ~10s of rows for most personal
    /// deployments; we don't paginate the trait surface but backends
    /// may impose internal caps.
    async fn list_collections(&self) -> RagResult<Vec<Collection>>;

    /// Run a retrieval query. The citation contract is the load-
    /// bearing assertion — every `SearchHit` must populate
    /// `source_uri + span + score`. Backends that can't compute lines
    /// MUST do so at ingest time.
    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult>;

    /// Index a document into the collection. Idempotent on
    /// `source_uri`: a re-ingest replaces the prior version (so
    /// folder watchers can re-fire on file change without
    /// accumulating dupes).
    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult>;

    /// Remove a document by id. Idempotent — missing is `Ok(())`.
    async fn delete_document(&self, collection_id: &str, document_id: &str) -> RagResult<()>;
}

// Object-safety check.
const _: Option<Box<dyn RagClient>> = None;
