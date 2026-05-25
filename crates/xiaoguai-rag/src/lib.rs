//! RAG layer for xiaoguai.
//!
//! v0.9.2 follows the roadmap principle "wrap a mature layer, don't
//! build our own": this crate is a thin adapter over one of two
//! backends —
//!
//! * [`R2RClient`] — HTTP adapter to SciPhi-AI's R2R, the recommended
//!   production target (knowledge graph, hybrid retrieval, Deep
//!   Research endpoint, multimodal ingest).
//! * [`InMemoryRagClient`] — substring-only dev/test fallback. Not
//!   intelligent; exists so the crate has a deterministic backend
//!   for unit tests and `xiaoguai-core` smoke runs without booting
//!   R2R.
//!
//! Both implement the same [`RagClient`] trait; production wiring
//! substitutes one for the other behind `Arc<dyn RagClient>` with no
//! call-site change.
//!
//! The crate also ships [`RagMcpAdapter`], which wraps any
//! `RagClient` and implements `xiaoguai_mcp::McpClient`. That makes
//! RAG show up in the `Toolbox` exactly like any other MCP server —
//! the agent loop calls `rag.search` / `rag.ingest` /
//! `rag.list_collections` / `rag.delete_doc` through the same
//! mechanism as a `filesystem` or `github` MCP tool.
//!
//! Citation contract (locked in the v0.9 roadmap): every search hit
//! carries `source_uri + span + score`. Adapters that can't produce
//! a span MUST compute one at ingest time from chunk offsets — no
//! silent unsourced text.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod client;
pub mod hybrid;
pub mod loaders;
pub mod memory;
pub mod presets;
pub mod qdrant;
pub mod r2r;
pub mod reranker;
pub mod tantivy_backend;
pub mod types;

pub use adapter::RagMcpAdapter;
pub use client::{RagClient, RagError, RagResult};
pub use hybrid::{rrf_fuse, HybridConfig, HybridRetriever, RankedHit};
pub use loaders::{
    detect, DocxLoader, HtmlLoader, LoadError, LoadResult, LoadedDoc, Loader, MarkdownLoader,
    PageMeta, PdfLoader, PptxLoader,
};
pub use memory::InMemoryRagClient;
pub use presets::{ChunkingPreset, IngestOptions};
pub use qdrant::{point_id_for, QdrantDistance, QdrantPoint, QdrantStore, VectorStore};
pub use r2r::R2RClient;
pub use reranker::{
    two_stage_retrieve, Candidate, CohereReranker, JinaReranker, LlmReranker, NullReranker,
    Reranker, RerankerConfig, Scored, VoyageReranker,
};
pub use tantivy_backend::{LexicalStore, TantivyStore};
pub use types::{
    Citation, Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};
