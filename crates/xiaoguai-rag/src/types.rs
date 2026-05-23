//! Wire types shared by every `RagClient` impl.
//!
//! The citation contract is the load-bearing part: every `SearchHit`
//! must populate `source_uri + span + score`. UI surfaces (chat-ui,
//! eval graders) and the v0.9.3 `ContentBlock::Citation` variant all
//! depend on this contract holding for every backend.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A retrieval collection — RAG-lingo for "namespace of indexed docs".
/// Multiple collections per tenant is supported; production usage is
/// typically one per knowledge domain (`obsidian-vault`, `repo-foo`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    pub document_count: u64,
    pub created_at: DateTime<Utc>,
}

/// Citation contract — every hit MUST populate this exactly. The
/// citation model is shared between RAG, the future
/// `ContentBlock::Citation`, and the eval graders, so it lives in
/// `xiaoguai-rag` only because that's where the first user lands. If
/// a second consumer grows, lift this into `xiaoguai-types`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Citation {
    /// `file://` for local docs, `https://` for remote, or a custom
    /// scheme registered by the connector (e.g. `obsidian://`).
    pub source_uri: String,
    /// Inclusive `[start, end]` line numbers (1-indexed). Backends
    /// that can't produce lines must compute them from chunk offsets
    /// at ingest time — see crate-level docs.
    pub span: (u32, u32),
    /// Retrieval score in `[0, 1]`. Used for tie-break in UI sort order
    /// and as a feature in eval graders. Backends normalise to [0,1].
    pub score: f32,
    /// The retrieved chunk text. Sized for hover-card preview (~200-
    /// 400 chars typical).
    pub preview: String,
    /// Provenance back to the collection so the UI can offer
    /// "find more from this source".
    pub collection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub collection_id: String,
    pub query: String,
    /// Default `8` is a community-validated balance for hybrid
    /// retrieval + reranker tuning. Hosts may override.
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    /// When `Some`, restrict to hits whose `score >= threshold`. Used
    /// by eval graders that want to detect "low-confidence drift"
    /// without re-running the LLM.
    pub min_score: Option<f32>,
}

fn default_top_k() -> u32 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub citation: Citation,
    /// Original document id (collection-scoped). Useful when the
    /// caller wants to re-fetch the full doc.
    pub document_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    /// How long the underlying backend took, in milliseconds.
    /// Surfaced by eval graders to detect performance regression.
    pub elapsed_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    pub collection_id: String,
    /// `file://` URI when known; otherwise a synthetic id the backend
    /// generates. Used as the document's primary `source_uri`.
    pub source_uri: String,
    /// Inline content. Binary inputs (PDFs, images) should be base64-
    /// encoded into a JSON blob upstream, since the trait is text-
    /// only by intent.
    pub content: String,
    /// Free-form metadata stored alongside the document and round-
    /// tripped on retrieval.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub document_id: String,
    pub chunk_count: u32,
}
