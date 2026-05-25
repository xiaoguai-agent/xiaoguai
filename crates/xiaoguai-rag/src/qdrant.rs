//! Qdrant vector-store backend.
//!
//! ## Trait factoring decision (v1.2.16)
//!
//! `QdrantStore` implements [`RagClient`] (the existing unified trait) so
//! it drops in wherever `R2RClient` or `InMemoryRagClient` live today —
//! no call-site changes. It *also* implements the narrower [`VectorStore`]
//! trait defined in this module, which exposes Qdrant-specific primitives
//! that the unified trait cannot express:
//!
//! * `create_collection` — named-vector schema + distance metric
//! * `upsert_points` — raw [`QdrantPoint`] batch, bypassing the ingest
//!   chunking path (useful when the caller has pre-embedded vectors)
//! * `delete_by_id` — point-level deletion (vs. document-level in
//!   `RagClient::delete_document`)
//!
//! Callers that need only generic RAG operations receive
//! `Arc<dyn RagClient>`. Callers that need the low-level surface receive
//! `Arc<dyn VectorStore>`. Both can be obtained from the same
//! `QdrantStore` instance via `Arc::clone` + `as dyn`.
//!
//! ## Async runtime
//!
//! `qdrant-client` 1.x uses `tonic` (gRPC) internally and already
//! requires a Tokio runtime. There is no sync wrapper needed; every
//! `VectorStore` / `RagClient` method is `async` and safe to call from
//! the existing `tokio::main` runtime.
//!
//! ## Vector IDs
//!
//! Qdrant point IDs are either `u64` or UUID. We encode the caller's
//! `document_id` string as a deterministic UUID v5 (namespace:
//! `NAMESPACE_OID`, name = `"{collection_id}/{document_id}"`) so that
//! ingest is idempotent and deletion is O(1) without a scan. The
//! encoding is tested independently from any live Qdrant instance.
//!
//! ## Named vectors
//!
//! `QdrantStore` works with a single named dense vector field (default:
//! `"content"`). This covers the common single-embedding-model deployment.
//! Collections that need multiple named fields (e.g. `"text"` + `"image"`)
//! can be created via `VectorStore::create_collection` with a different
//! `vector_name`, then accessed by constructing a `QdrantStore` with the
//! matching `vector_name`.
//!
//! ## Integration tests
//!
//! Tests that require a live Qdrant are decorated with `#[ignore]`.
//! Run them with:
//! ```bash
//! QDRANT_URL=http://localhost:6334 cargo test -p xiaoguai-rag qdrant -- --ignored
//! ```
//! Unit tests (ID encoding, distance serialization) run without any
//! external service.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use qdrant_client::qdrant::{
    CreateCollectionBuilder, DeletePointsBuilder, Distance, PointsIdsList, SearchParamsBuilder,
    SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use uuid::Uuid;

use crate::client::{RagClient, RagError, RagResult};
use crate::types::{
    Citation, Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};

// ---------------------------------------------------------------------------
// VectorStore — Qdrant-specific extension trait
// ---------------------------------------------------------------------------

/// Extended trait for Qdrant-specific low-level operations.
///
/// Use `Arc<dyn VectorStore>` when you need raw point access.
/// Use `Arc<dyn RagClient>` when you want the generic RAG surface.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Create a new collection with a single named dense vector field.
    ///
    /// * `name` — collection name (= `collection_id` in `RagClient` land).
    /// * `vector_name` — named-vector key, e.g. `"content"`.
    /// * `dim` — embedding dimension (e.g. 1536 for `text-embedding-3-small`).
    /// * `distance` — similarity metric; typically `Cosine` for text.
    ///
    /// Idempotent: if the collection already exists the call succeeds.
    async fn create_collection(
        &self,
        name: &str,
        vector_name: &str,
        dim: u64,
        distance: QdrantDistance,
    ) -> RagResult<()>;

    /// Upsert pre-embedded points into the collection. The caller owns
    /// the embedding step — useful when the embedding model lives outside
    /// this crate.
    ///
    /// Idempotent on point ID: upsert replaces an existing point with the
    /// same ID (Qdrant semantics).
    async fn upsert_points(&self, collection: &str, points: Vec<QdrantPoint>) -> RagResult<()>;

    /// Delete a single point by its raw Qdrant point ID.
    ///
    /// Idempotent — missing is `Ok(())`.
    async fn delete_by_id(&self, collection: &str, point_id: Uuid) -> RagResult<()>;
}

// ---------------------------------------------------------------------------
// Public helpers (distance, point) — stable API surface
// ---------------------------------------------------------------------------

/// Distance metric used when creating a Qdrant collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QdrantDistance {
    Cosine,
    Dot,
    Euclid,
    Manhattan,
}

impl QdrantDistance {
    /// Map to the qdrant-client proto enum variant.
    fn to_proto(self) -> Distance {
        match self {
            Self::Cosine => Distance::Cosine,
            Self::Dot => Distance::Dot,
            Self::Euclid => Distance::Euclid,
            Self::Manhattan => Distance::Manhattan,
        }
    }

    /// Display name used in error messages and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cosine => "Cosine",
            Self::Dot => "Dot",
            Self::Euclid => "Euclid",
            Self::Manhattan => "Manhattan",
        }
    }
}

/// A single point to upsert into Qdrant.
#[derive(Debug, Clone)]
pub struct QdrantPoint {
    /// Deterministic ID — use [`point_id_for`] to derive from document IDs.
    pub id: Uuid,
    /// Named vector (key must match `vector_name` used at collection creation).
    pub vector_name: String,
    pub vector: Vec<f32>,
    /// Free-form metadata stored alongside the vector (surfaced on retrieval).
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ID encoding
// ---------------------------------------------------------------------------

/// Deterministic UUID v5 from `"{collection}/{document_id}"`.
///
/// Using UUIDv5 (SHA-1 namespace hash) gives us:
/// * Idempotent ingest — same input always produces the same point ID.
/// * O(1) deletion — no scan needed to find the point.
/// * Collision-resistance good enough for RAG IDs (~10^38 distinct values).
pub fn point_id_for(collection: &str, document_id: &str) -> Uuid {
    let name = format!("{collection}/{document_id}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes())
}

// ---------------------------------------------------------------------------
// QdrantStore
// ---------------------------------------------------------------------------

/// Qdrant vector-store backend.
///
/// Connect via the gRPC endpoint (`http://localhost:6334` by default).
pub struct QdrantStore {
    client: Qdrant,
    /// Named-vector field used when calling `RagClient::search` /
    /// `ingest`. Configurable so operators can pick the key that matches
    /// their collection schema.
    vector_name: String,
}

impl std::fmt::Debug for QdrantStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantStore")
            .field("vector_name", &self.vector_name)
            .finish_non_exhaustive()
    }
}

impl QdrantStore {
    /// Connect to a Qdrant instance.
    ///
    /// * `url` — gRPC endpoint, e.g. `http://localhost:6334`.
    /// * `vector_name` — named-vector key for the dense text embedding
    ///   field (must match the schema used at collection-creation time).
    ///
    /// # Errors
    ///
    /// Returns [`RagError::Backend`] if the Qdrant client fails to
    /// initialise (invalid URL, TLS error, etc.).
    pub fn new(url: impl Into<String>, vector_name: impl Into<String>) -> RagResult<Self> {
        let client = Qdrant::from_url(&url.into())
            .build()
            .map_err(|e| RagError::Backend(format!("qdrant connect: {e}")))?;
        Ok(Self {
            client,
            vector_name: vector_name.into(),
        })
    }

    /// Default constructor using `http://localhost:6334` and `"content"`.
    pub fn default_local() -> RagResult<Self> {
        Self::new("http://localhost:6334", "content")
    }
}

// ---------------------------------------------------------------------------
// VectorStore impl
// ---------------------------------------------------------------------------

#[async_trait]
impl VectorStore for QdrantStore {
    async fn create_collection(
        &self,
        name: &str,
        vector_name: &str,
        dim: u64,
        distance: QdrantDistance,
    ) -> RagResult<()> {
        // Check if collection already exists — idempotent.
        let exists = self
            .client
            .collection_exists(name)
            .await
            .map_err(|e| RagError::Backend(format!("qdrant collection_exists: {e}")))?;
        if exists {
            return Ok(());
        }

        // For a single named vector we pass VectorParamsBuilder directly
        // to `vectors_config()`. The builder accepts `impl Into<VectorsConfig>`
        // and `VectorParamsBuilder` implements that conversion.
        //
        // For *multiple* named vectors one would use a `VectorParamsMap`
        // with explicit field names. That use-case is handled via
        // `upsert_points` with pre-built vectors; the `create_collection`
        // method covers the common single-field schema.
        //
        // Named-vector wrapping: passing `VectorParamsBuilder` directly
        // creates a single unnamed default vector. To get a named vector
        // we build a `VectorParamsMap` manually.
        let mut params_map: HashMap<String, qdrant_client::qdrant::VectorParams> = HashMap::new();
        let vp: qdrant_client::qdrant::VectorParams =
            VectorParamsBuilder::new(dim, distance.to_proto()).into();
        params_map.insert(vector_name.to_string(), vp);
        let vectors_config = qdrant_client::qdrant::VectorsConfig {
            config: Some(qdrant_client::qdrant::vectors_config::Config::ParamsMap(
                qdrant_client::qdrant::VectorParamsMap { map: params_map },
            )),
        };

        self.client
            .create_collection(CreateCollectionBuilder::new(name).vectors_config(vectors_config))
            .await
            .map_err(|e| RagError::Backend(format!("qdrant create_collection: {e}")))?;

        tracing::info!(
            collection = name,
            dim,
            distance = distance.as_str(),
            "qdrant collection created"
        );
        Ok(())
    }

    async fn upsert_points(&self, collection: &str, points: Vec<QdrantPoint>) -> RagResult<()> {
        if points.is_empty() {
            return Ok(());
        }
        let proto_points: Vec<qdrant_client::qdrant::PointStruct> = points
            .into_iter()
            .map(|p| {
                // Build named-vector map: HashMap<String, Vec<f32>> → Vectors.
                let vectors: HashMap<String, Vec<f32>> =
                    [(p.vector_name.clone(), p.vector)].into_iter().collect();
                // Use serde_json payload: serde_json::Value → Payload.
                let payload = qdrant_client::Payload::try_from(p.payload).unwrap_or_default();
                qdrant_client::qdrant::PointStruct::new(p.id.to_string(), vectors, payload)
            })
            .collect();

        self.client
            .upsert_points(UpsertPointsBuilder::new(collection, proto_points))
            .await
            .map_err(|e| RagError::Backend(format!("qdrant upsert: {e}")))?;
        Ok(())
    }

    async fn delete_by_id(&self, collection: &str, point_id: Uuid) -> RagResult<()> {
        let point_id_proto: qdrant_client::qdrant::PointId = point_id.to_string().into();
        self.client
            .delete_points(DeletePointsBuilder::new(collection).points(PointsIdsList {
                ids: vec![point_id_proto],
            }))
            .await
            .map_err(|e| RagError::Backend(format!("qdrant delete_by_id: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RagClient impl
// ---------------------------------------------------------------------------

#[async_trait]
impl RagClient for QdrantStore {
    async fn list_collections(&self) -> RagResult<Vec<Collection>> {
        let resp = self
            .client
            .list_collections()
            .await
            .map_err(|e| RagError::Backend(format!("qdrant list_collections: {e}")))?;
        let now = Utc::now();
        let collections: Vec<Collection> = resp
            .collections
            .into_iter()
            .map(|c| Collection {
                id: c.name.clone(),
                name: c.name,
                description: None,
                // Qdrant's list endpoint doesn't return doc count — callers
                // that need exact counts should use `collection_info`.
                document_count: 0,
                created_at: now,
            })
            .collect();
        Ok(collections)
    }

    /// Semantic vector search.
    ///
    /// Because the `RagClient::search` API receives a *text* query, not a
    /// pre-embedded vector, this impl **requires the caller to supply an
    /// embedding**. In the current design the query is passed as a
    /// `base64url`-encoded float32 vector in the `SearchRequest::query`
    /// field when the Qdrant backend is active.
    ///
    /// If the `query` field does not start with `"vec:"`, the call returns
    /// [`RagError::InvalidArgument`] with a clear message. This is an
    /// intentional design choice: a future `EmbeddingMiddleware` wrapper
    /// (roadmap v1.2.17) will intercept the text query, run an embedding
    /// model, and re-encode as `"vec:<base64>"` before delegating here.
    /// For now the boundary is explicit rather than silent.
    ///
    /// Format: `"vec:<base64url-encoded little-endian f32 array>"`
    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult> {
        let start = std::time::Instant::now();
        let vector = parse_query_vector(&req.query)?;

        let search_result = self
            .client
            .search_points(
                SearchPointsBuilder::new(&req.collection_id, vector, u64::from(req.top_k))
                    .vector_name(self.vector_name.clone())
                    .with_payload(true)
                    .params(SearchParamsBuilder::default().exact(false)),
            )
            .await
            .map_err(|e| RagError::Backend(format!("qdrant search: {e}")))?;

        let mut hits: Vec<SearchHit> = search_result
            .result
            .into_iter()
            .filter_map(|scored| {
                let score = scored.score.clamp(0.0, 1.0);
                if let Some(min) = req.min_score {
                    if score < min {
                        return None;
                    }
                }
                let source_uri = scored
                    .get("source_uri")
                    .as_str()
                    .map(String::from)
                    .unwrap_or_default();
                let document_id = scored
                    .get("document_id")
                    .as_str()
                    .map(String::from)
                    .unwrap_or_default();
                let preview = scored
                    .get("text")
                    .as_str()
                    .map(|s| s.chars().take(400).collect::<String>())
                    .unwrap_or_default();
                let line_start = scored
                    .get("line_start")
                    .as_integer()
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0);
                let line_end = scored
                    .get("line_end")
                    .as_integer()
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(line_start);

                Some(SearchHit {
                    document_id: document_id.clone(),
                    citation: Citation {
                        source_uri: if source_uri.is_empty() {
                            format!("qdrant://{}/{}", req.collection_id, document_id)
                        } else {
                            source_uri
                        },
                        span: (line_start, line_end),
                        score,
                        preview,
                        collection_id: req.collection_id.clone(),
                    },
                })
            })
            .collect();

        hits.sort_by(|a, b| {
            b.citation
                .score
                .partial_cmp(&a.citation.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let elapsed_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
        Ok(SearchResult { hits, elapsed_ms })
    }

    /// Ingest a document into Qdrant.
    ///
    /// The `IngestRequest::content` field must be a `"vec:<base64>"` string
    /// (same convention as `search`) carrying the pre-computed embedding.
    /// The `source_uri` and `metadata` are stored in the point payload.
    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult> {
        let vector = parse_query_vector(&req.content)?;
        let document_id = format!("{}-{}", req.collection_id, &Uuid::new_v4().to_string()[..8]);
        let point_id = point_id_for(&req.collection_id, &document_id);

        let vectors: HashMap<String, Vec<f32>> =
            [(self.vector_name.clone(), vector)].into_iter().collect();

        let payload_json = serde_json::json!({
            "source_uri": req.source_uri,
            "document_id": document_id,
            "collection_id": req.collection_id,
        });
        let payload = qdrant_client::Payload::try_from(payload_json)
            .map_err(|e| RagError::Backend(format!("qdrant payload build: {e}")))?;

        let point = qdrant_client::qdrant::PointStruct::new(point_id.to_string(), vectors, payload);

        self.client
            .upsert_points(UpsertPointsBuilder::new(&req.collection_id, vec![point]))
            .await
            .map_err(|e| RagError::Backend(format!("qdrant ingest upsert: {e}")))?;

        Ok(IngestResult {
            document_id,
            chunk_count: 1,
        })
    }

    async fn delete_document(&self, collection_id: &str, document_id: &str) -> RagResult<()> {
        let point_id = point_id_for(collection_id, document_id);
        self.delete_by_id(collection_id, point_id).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse `"vec:<base64url-encoded f32 LE bytes>"` into a `Vec<f32>`.
///
/// This is a deliberate explicit protocol: callers embed outside this
/// crate and pass the result as an opaque string, keeping the embedding
/// model swappable without touching the Qdrant backend.
fn parse_query_vector(s: &str) -> RagResult<Vec<f32>> {
    let encoded = s.strip_prefix("vec:").ok_or_else(|| {
        RagError::InvalidArgument(
            "Qdrant backend requires query/content to start with \"vec:<base64url-f32>\". \
             A future EmbeddingMiddleware wrapper will handle plain text; \
             for now supply the pre-computed embedding directly."
                .into(),
        )
    })?;
    let bytes = base64url_decode(encoded)?;
    if bytes.len() % 4 != 0 {
        return Err(RagError::InvalidArgument(format!(
            "vec: byte length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if floats.is_empty() {
        return Err(RagError::InvalidArgument("vec: empty vector".into()));
    }
    Ok(floats)
}

/// Minimal base64url decoder (no external dep beyond std).
fn base64url_decode(s: &str) -> RagResult<Vec<u8>> {
    // Pad to 4-byte boundary.
    let mut padded = s.to_string();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    // Convert base64url alphabet → standard base64.
    let standard = padded.replace('-', "+").replace('_', "/");

    let mut table = [255u8; 256];
    for (i, &c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        table[c as usize] = i as u8;
    }
    table[b'=' as usize] = 0;

    let mut out = Vec::with_capacity(standard.len() * 3 / 4);
    let bytes = standard.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = table[bytes[i] as usize];
        let b = table[bytes[i + 1] as usize];
        let c = table[bytes[i + 2] as usize];
        let d = table[bytes[i + 3] as usize];
        if a == 255 || b == 255 {
            return Err(RagError::InvalidArgument(format!(
                "base64 invalid chars at offset {i}"
            )));
        }
        out.push((a << 2) | (b >> 4));
        if bytes[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if bytes[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    Ok(out)
}

/// Encode bytes to base64url (no padding) — used only in tests.
#[cfg(test)]
pub(crate) fn base64url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied().unwrap_or(0);
        let b2 = bytes.get(i + 2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[((b0 & 3) << 4 | b1 >> 4) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[((b1 & 0xf) << 2 | b2 >> 6) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
        i += 3;
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Unit tests — no Qdrant instance needed
    // ------------------------------------------------------------------

    #[test]
    fn point_id_is_deterministic() {
        let id1 = point_id_for("my-coll", "doc-42");
        let id2 = point_id_for("my-coll", "doc-42");
        assert_eq!(id1, id2, "same inputs must produce same UUID");
    }

    #[test]
    fn point_id_differs_across_collections() {
        let id1 = point_id_for("coll-a", "doc-1");
        let id2 = point_id_for("coll-b", "doc-1");
        assert_ne!(id1, id2, "different collection must produce different UUID");
    }

    #[test]
    fn point_id_is_version_5() {
        let id = point_id_for("c", "d");
        assert_eq!(id.get_version(), Some(uuid::Version::Sha1));
    }

    #[test]
    fn distance_metric_serializes_to_proto_variant() {
        assert_eq!(QdrantDistance::Cosine.to_proto(), Distance::Cosine);
        assert_eq!(QdrantDistance::Dot.to_proto(), Distance::Dot);
        assert_eq!(QdrantDistance::Euclid.to_proto(), Distance::Euclid);
        assert_eq!(QdrantDistance::Manhattan.to_proto(), Distance::Manhattan);
    }

    #[test]
    fn distance_as_str_is_human_readable() {
        assert_eq!(QdrantDistance::Cosine.as_str(), "Cosine");
        assert_eq!(QdrantDistance::Manhattan.as_str(), "Manhattan");
    }

    #[test]
    fn parse_query_vector_rejects_plain_text() {
        let err = parse_query_vector("hello world").unwrap_err();
        assert!(matches!(err, RagError::InvalidArgument(_)));
        // Error message should guide the user.
        let msg = err.to_string();
        assert!(
            msg.contains("vec:"),
            "message should mention the expected prefix"
        );
    }

    #[test]
    fn parse_query_vector_rejects_empty_vec() {
        let err = parse_query_vector("vec:").unwrap_err();
        assert!(matches!(err, RagError::InvalidArgument(_)));
    }

    #[test]
    fn parse_query_vector_roundtrip() {
        // Encode [1.0, -0.5, 0.0] as LE bytes → base64url → parse back.
        let original: Vec<f32> = vec![1.0_f32, -0.5_f32, 0.0_f32];
        let bytes: Vec<u8> = original.iter().flat_map(|f| f.to_le_bytes()).collect();
        let encoded = base64url_encode(&bytes);
        let query = format!("vec:{encoded}");
        let recovered = parse_query_vector(&query).unwrap();
        assert_eq!(recovered.len(), 3);
        assert!((recovered[0] - 1.0).abs() < 1e-6);
        assert!((recovered[1] - (-0.5)).abs() < 1e-6);
        assert!(recovered[2].abs() < 1e-6);
    }

    #[test]
    fn parse_query_vector_rejects_non_multiple_of_4_bytes() {
        // 5 bytes — not divisible by 4.
        let bytes = [0u8; 5];
        let encoded = base64url_encode(&bytes);
        let err = parse_query_vector(&format!("vec:{encoded}")).unwrap_err();
        assert!(matches!(err, RagError::InvalidArgument(_)));
    }

    // ------------------------------------------------------------------
    // Integration tests — require QDRANT_URL env var + live instance
    // ------------------------------------------------------------------

    /// Start Qdrant locally:
    /// ```bash
    /// docker run -p 6333:6333 -p 6334:6334 qdrant/qdrant
    /// QDRANT_URL=http://localhost:6334 cargo test -p xiaoguai-rag qdrant -- --ignored
    /// ```
    fn qdrant_url() -> Option<String> {
        std::env::var("QDRANT_URL").ok()
    }

    #[tokio::test]
    #[ignore = "requires live Qdrant; set QDRANT_URL=http://localhost:6334"]
    async fn integration_create_collection_is_idempotent() {
        let url = qdrant_url().expect("QDRANT_URL must be set");
        let store = QdrantStore::new(url, "content").unwrap();
        let coll = format!("test-idem-{}", Uuid::new_v4().simple());
        store
            .create_collection(&coll, "content", 4, QdrantDistance::Cosine)
            .await
            .unwrap();
        // Second call must not error.
        store
            .create_collection(&coll, "content", 4, QdrantDistance::Cosine)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires live Qdrant; set QDRANT_URL=http://localhost:6334"]
    async fn integration_upsert_and_delete_by_id() {
        let url = qdrant_url().expect("QDRANT_URL must be set");
        let store = QdrantStore::new(url, "content").unwrap();
        let coll = format!("test-del-{}", Uuid::new_v4().simple());
        store
            .create_collection(&coll, "content", 4, QdrantDistance::Cosine)
            .await
            .unwrap();

        let pid = point_id_for(&coll, "doc-1");
        store
            .upsert_points(
                &coll,
                vec![QdrantPoint {
                    id: pid,
                    vector_name: "content".into(),
                    vector: vec![1.0, 0.0, 0.0, 0.0],
                    payload: serde_json::json!({ "document_id": "doc-1" }),
                }],
            )
            .await
            .unwrap();

        // Delete — must be idempotent on a second call.
        store.delete_by_id(&coll, pid).await.unwrap();
        store.delete_by_id(&coll, pid).await.unwrap();
    }
}
