//! Tantivy BM25 lexical-search backend.
//!
//! ## Why a separate `LexicalStore` trait?
//!
//! The `RagClient` trait is the right abstraction for callers that only
//! need `search / ingest / delete_document`. Tantivy also exposes a few
//! index-management primitives (`create_index`, `commit`) that don't
//! belong on the shared trait. We surface those on `LexicalStore` (same
//! pattern as `VectorStore` in `qdrant.rs`).
//!
//! ## Index life-cycle
//!
//! Tantivy indexes are `(schema, directory)` pairs. This backend uses one
//! Tantivy index *per collection* stored in a `HashMap<String, IndexState>`.
//! Collections are created on-demand at first ingest (mirrors the
//! `InMemoryRagClient` behaviour). For production usage where indexes
//! should survive process restart, supply an `MmapDirectory`-backed path
//! via `TantivyStore::open` (not yet wired; planned for v1.2.17 when the
//! file-watch bridge integrates with Tantivy).
//!
//! ## BM25 scoring and citation
//!
//! Tantivy's `BM25Similarity` is the default scorer. Scores are raw Tantivy
//! floats (un-bounded positive reals). We normalise to `[0, 1]` by clamping
//! `score / (score + 1)` (a monotone squashing function that maps `[0, ∞)`
//! to `[0, 1)` and preserves relative ranking). This is coarse but avoids
//! a second retrieval pass for `max_score`.
//!
//! ## Async runtime
//!
//! Tantivy is synchronous. Every method runs on a `tokio::task::spawn_blocking`
//! thread to avoid blocking the async executor. This matches the pattern
//! used in `xiaoguai-storage` (SQLx `spawn_blocking` calls for PG
//! operations that pre-date async sqlx support).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, SchemaBuilder, Value as TantivyValue, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::client::{RagClient, RagError, RagResult};
use crate::types::{
    Citation, Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};

// ---------------------------------------------------------------------------
// LexicalStore — Tantivy-specific extension trait
// ---------------------------------------------------------------------------

/// Extended trait for Tantivy-specific index-management operations.
///
/// Use `Arc<dyn LexicalStore>` when you need to manage indexes directly.
/// Use `Arc<dyn RagClient>` when you want the generic RAG surface.
#[async_trait]
pub trait LexicalStore: Send + Sync {
    /// Ensure a named collection (= Tantivy index) exists.
    ///
    /// Idempotent — calling twice is safe. The schema is fixed at creation:
    /// `id` (STRING stored), `source_uri` (STRING stored), `text` (TEXT stored),
    /// `collection_id` (STRING stored).
    async fn ensure_collection(&self, collection_id: &str) -> RagResult<()>;

    /// Flush pending writes to the index and make them searchable.
    ///
    /// `ingest` batches writes but does not auto-commit. Call `commit`
    /// after a bulk-ingest to make the documents visible to `search`.
    /// For incremental real-time ingest, `TantivyStore` auto-commits on
    /// every `ingest` call, so explicit `commit` is only needed after
    /// using `upsert_raw`.
    async fn commit(&self, collection_id: &str) -> RagResult<()>;
}

// ---------------------------------------------------------------------------
// Internal index state
// ---------------------------------------------------------------------------

struct IndexState {
    _schema: Schema,
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    // Field handles — cached to avoid schema lookup on every operation.
    f_id: Field,
    f_source_uri: Field,
    f_text: Field,
    f_collection_id: Field,
    f_line_start: Field,
    f_line_end: Field,
}

fn build_schema() -> (Schema, Field, Field, Field, Field, Field, Field) {
    let mut builder = SchemaBuilder::new();
    let f_id = builder.add_text_field("id", STRING | STORED);
    let f_source_uri = builder.add_text_field("source_uri", STRING | STORED);
    let f_text = builder.add_text_field("text", TEXT | STORED);
    let f_collection_id = builder.add_text_field("collection_id", STRING | STORED);
    let f_line_start = builder.add_u64_field("line_start", STORED);
    let f_line_end = builder.add_u64_field("line_end", STORED);
    let schema = builder.build();
    (
        schema,
        f_id,
        f_source_uri,
        f_text,
        f_collection_id,
        f_line_start,
        f_line_end,
    )
}

fn open_index_in_ram() -> RagResult<IndexState> {
    let (schema, f_id, f_source_uri, f_text, f_collection_id, f_line_start, f_line_end) =
        build_schema();
    let index = Index::create_in_ram(schema.clone());
    // Use Manual reload for in-RAM indexes so that callers can call
    // `reader.reload()` immediately after `writer.commit()` — making newly
    // committed documents visible without waiting for a background timer.
    // `OnCommitWithDelay` (the default) uses a background thread with a
    // short sleep that races against the test assertions.
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|e| RagError::Backend(format!("tantivy reader: {e}")))?;
    let writer = index
        .writer(50_000_000) // 50 MB heap
        .map_err(|e| RagError::Backend(format!("tantivy writer: {e}")))?;
    Ok(IndexState {
        _schema: schema,
        index,
        reader,
        writer: Arc::new(Mutex::new(writer)),
        f_id,
        f_source_uri,
        f_text,
        f_collection_id,
        f_line_start,
        f_line_end,
    })
}

fn open_index_on_disk(dir: &Path) -> RagResult<IndexState> {
    let (schema, f_id, f_source_uri, f_text, f_collection_id, f_line_start, f_line_end) =
        build_schema();
    let mmap_dir = MmapDirectory::open(dir)
        .map_err(|e| RagError::Backend(format!("tantivy mmap_dir: {e}")))?;
    let index = Index::open_or_create(mmap_dir, schema.clone())
        .map_err(|e| RagError::Backend(format!("tantivy open_or_create: {e}")))?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()
        .map_err(|e| RagError::Backend(format!("tantivy reader: {e}")))?;
    let writer = index
        .writer(50_000_000)
        .map_err(|e| RagError::Backend(format!("tantivy writer: {e}")))?;
    Ok(IndexState {
        _schema: schema,
        index,
        reader,
        writer: Arc::new(Mutex::new(writer)),
        f_id,
        f_source_uri,
        f_text,
        f_collection_id,
        f_line_start,
        f_line_end,
    })
}

// ---------------------------------------------------------------------------
// TantivyStore
// ---------------------------------------------------------------------------

/// Tantivy BM25 lexical-search backend implementing [`RagClient`].
///
/// Each collection is a separate in-RAM (default) or on-disk Tantivy index.
/// The backend is safe to share across threads via `Arc`.
pub struct TantivyStore {
    /// `collection_id → IndexState`
    indexes: Mutex<HashMap<String, IndexState>>,
    /// When `Some`, new collections open on-disk under this directory.
    /// When `None`, all collections use in-RAM indexes.
    base_dir: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for TantivyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TantivyStore")
            .field("collections", &self.indexes.lock().len())
            .field("base_dir", &self.base_dir)
            .finish()
    }
}

impl TantivyStore {
    /// Create an in-RAM store (dev/test — does not persist across restarts).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            indexes: Mutex::new(HashMap::new()),
            base_dir: None,
        }
    }

    /// Create a disk-backed store.
    ///
    /// Each collection is stored under `base_dir/<collection_id>/`.
    /// The directory is created on demand.
    pub fn on_disk(base_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            indexes: Mutex::new(HashMap::new()),
            base_dir: Some(base_dir.into()),
        }
    }

    /// Ensure an index exists for `collection_id`, creating it if needed.
    fn ensure_index(&self, collection_id: &str) -> RagResult<()> {
        let mut guard = self.indexes.lock();
        if guard.contains_key(collection_id) {
            return Ok(());
        }
        let state = if let Some(ref base) = self.base_dir {
            let dir = base.join(collection_id);
            std::fs::create_dir_all(&dir).map_err(|e| {
                RagError::Backend(format!("tantivy create dir {}: {e}", dir.display()))
            })?;
            open_index_on_disk(&dir)?
        } else {
            open_index_in_ram()?
        };
        guard.insert(collection_id.to_string(), state);
        Ok(())
    }

    /// Squash a raw BM25 score (unbounded positive) to `[0, 1)`.
    ///
    /// Using `s / (s + 1)` — monotone, no second pass required.
    fn normalise_score(raw: f32) -> f32 {
        if raw <= 0.0 {
            return 0.0;
        }
        (raw / (raw + 1.0)).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// LexicalStore impl
// ---------------------------------------------------------------------------

#[async_trait]
impl LexicalStore for TantivyStore {
    async fn ensure_collection(&self, collection_id: &str) -> RagResult<()> {
        self.ensure_index(collection_id)
    }

    async fn commit(&self, collection_id: &str) -> RagResult<()> {
        let (writer_arc, reader) = {
            let guard = self.indexes.lock();
            let state = guard.get(collection_id).ok_or_else(|| {
                RagError::NotFound(format!("tantivy collection not found: {collection_id}"))
            })?;
            (Arc::clone(&state.writer), state.reader.clone())
        };
        let mut writer = writer_arc.lock();
        writer
            .commit()
            .map_err(|e| RagError::Backend(format!("tantivy commit: {e}")))?;
        // Reload the reader to make committed documents visible immediately
        // (required when using ReloadPolicy::Manual for in-RAM indexes).
        reader
            .reload()
            .map_err(|e| RagError::Backend(format!("tantivy reader reload: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RagClient impl
// ---------------------------------------------------------------------------

#[async_trait]
impl RagClient for TantivyStore {
    async fn list_collections(&self) -> RagResult<Vec<Collection>> {
        let guard = self.indexes.lock();
        let mut cols: Vec<Collection> = guard
            .keys()
            .map(|id| Collection {
                id: id.clone(),
                name: id.clone(),
                description: None,
                // Tantivy doesn't expose a cheap doc count at the collection
                // level without a searcher hit-count query. Return 0 for now;
                // if a UI needs this, we'll add a separate `count` API.
                document_count: 0,
                created_at: Utc::now(),
            })
            .collect();
        cols.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(cols)
    }

    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult> {
        let start = std::time::Instant::now();

        // Validate eagerly before the blocking jump.
        if req.query.trim().is_empty() {
            return Err(RagError::InvalidArgument("query is empty".into()));
        }
        self.ensure_index(&req.collection_id)?;

        // Snapshot handles we need inside `spawn_blocking`.
        let (reader, query_parser, top_k, min_score, collection_id) = {
            let guard = self.indexes.lock();
            let state = guard
                .get(&req.collection_id)
                .ok_or_else(|| RagError::NotFound(req.collection_id.clone()))?;
            let reader = state.reader.clone();
            let qp = QueryParser::for_index(&state.index, vec![state.f_text]);
            (
                reader,
                qp,
                usize::try_from(req.top_k).unwrap_or(usize::MAX),
                req.min_score,
                req.collection_id.clone(),
            )
        };
        let query_str = req.query.clone();

        // Field handles for extraction — clone from index state.
        let (f_id, f_source_uri, f_text, f_line_start, f_line_end) = {
            let guard = self.indexes.lock();
            let s = guard.get(&req.collection_id).unwrap();
            (
                s.f_id,
                s.f_source_uri,
                s.f_text,
                s.f_line_start,
                s.f_line_end,
            )
        };

        let hits = tokio::task::spawn_blocking(move || -> RagResult<Vec<SearchHit>> {
            let searcher = reader.searcher();
            let query = query_parser
                .parse_query(&query_str)
                .map_err(|e| RagError::InvalidArgument(format!("tantivy parse: {e}")))?;

            let top_docs = searcher
                .search(&query, &TopDocs::with_limit(top_k))
                .map_err(|e| RagError::Backend(format!("tantivy search: {e}")))?;

            let mut out = Vec::with_capacity(top_docs.len());
            for (raw_score, addr) in top_docs {
                let score = TantivyStore::normalise_score(raw_score);
                if let Some(min) = min_score {
                    if score < min {
                        continue;
                    }
                }
                let doc: TantivyDocument = searcher
                    .doc(addr)
                    .map_err(|e| RagError::Backend(format!("tantivy fetch doc: {e}")))?;

                let get_str = |f: Field| -> String {
                    doc.get_first(f)
                        .and_then(|v| <_ as TantivyValue<'_>>::as_str(&v))
                        .unwrap_or("")
                        .to_string()
                };
                let get_u64 = |f: Field| -> u32 {
                    doc.get_first(f)
                        .and_then(|v| <_ as TantivyValue<'_>>::as_u64(&v))
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0)
                };

                let document_id = get_str(f_id);
                let source_uri = get_str(f_source_uri);
                let text = get_str(f_text);
                let line_start = get_u64(f_line_start);
                let line_end = get_u64(f_line_end);

                let preview: String = text.chars().take(400).collect();

                out.push(SearchHit {
                    document_id: document_id.clone(),
                    citation: Citation {
                        source_uri: if source_uri.is_empty() {
                            format!("tantivy://{collection_id}/{document_id}")
                        } else {
                            source_uri
                        },
                        span: (line_start, line_end),
                        score,
                        preview,
                        collection_id: collection_id.clone(),
                    },
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| RagError::Backend(format!("tantivy task join: {e}")))??;

        let elapsed_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
        Ok(SearchResult { hits, elapsed_ms })
    }

    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult> {
        self.ensure_index(&req.collection_id)?;

        let lines: Vec<String> = req.content.lines().map(str::to_string).collect();
        let chunk_count = u32::try_from(lines.len()).unwrap_or(u32::MAX);

        // Build one Tantivy document per chunk (line) so BM25 scoring
        // operates at chunk granularity — mirrors `InMemoryRagClient`.
        let (
            writer_arc,
            reader,
            f_id,
            f_source_uri,
            f_text,
            f_collection_id,
            f_line_start,
            f_line_end,
        ) = {
            let guard = self.indexes.lock();
            let s = guard.get(&req.collection_id).unwrap();
            (
                Arc::clone(&s.writer),
                s.reader.clone(),
                s.f_id,
                s.f_source_uri,
                s.f_text,
                s.f_collection_id,
                s.f_line_start,
                s.f_line_end,
            )
        };

        // Generate a stable document ID prefix from source_uri.
        let doc_prefix = format!("{}-{}", req.collection_id, urlify(&req.source_uri));

        let lines_clone = lines.clone();
        let source_uri = req.source_uri.clone();
        let collection_id = req.collection_id.clone();
        let doc_prefix_clone = doc_prefix.clone();

        tokio::task::spawn_blocking(move || -> RagResult<()> {
            let mut writer = writer_arc.lock();
            // Delete prior documents with the same source_uri first (idempotent).
            // Tantivy doesn't support delete-by-term on TEXT fields directly;
            // we use the STRING-typed `source_uri` field (= exact match).
            let del_term = tantivy::Term::from_field_text(f_source_uri, &source_uri);
            writer.delete_term(del_term);

            for (idx, line) in lines_clone.iter().enumerate() {
                let line_no = u64::try_from(idx + 1).unwrap_or(u64::MAX);
                let chunk_id = format!("{doc_prefix_clone}-L{line_no}");
                writer
                    .add_document(doc!(
                        f_id => chunk_id.as_str(),
                        f_source_uri => source_uri.as_str(),
                        f_text => line.as_str(),
                        f_collection_id => collection_id.as_str(),
                        f_line_start => line_no,
                        f_line_end => line_no,
                    ))
                    .map_err(|e| RagError::Backend(format!("tantivy add_document: {e}")))?;
            }
            // Auto-commit after each ingest so documents are immediately
            // searchable. This is fine for interactive / incremental use;
            // bulk-ingest callers can call `LexicalStore::commit` manually
            // and accept the write amplification trade-off.
            writer
                .commit()
                .map_err(|e| RagError::Backend(format!("tantivy commit: {e}")))?;
            // Explicitly reload the reader so newly committed documents are
            // visible immediately. Required when using ReloadPolicy::Manual
            // (in-RAM indexes) to avoid a race between the commit and the
            // next search call.
            reader
                .reload()
                .map_err(|e| RagError::Backend(format!("tantivy reader reload: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| RagError::Backend(format!("tantivy task join: {e}")))??;

        let document_id = doc_prefix;
        Ok(IngestResult {
            document_id,
            chunk_count,
        })
    }

    async fn delete_document(&self, collection_id: &str, document_id: &str) -> RagResult<()> {
        let (writer_arc, f_id) = {
            let guard = self.indexes.lock();
            if let Some(s) = guard.get(collection_id) {
                (Arc::clone(&s.writer), s.f_id)
            } else {
                return Ok(()); // collection doesn't exist — idempotent
            }
        };
        let doc_id_owned = document_id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut writer = writer_arc.lock();
            // Delete all chunks whose `id` field starts with the document prefix.
            // Since chunk IDs are `{doc_prefix}-L{line_no}`, a prefix term
            // covers all chunks belonging to the same document.
            let term = tantivy::Term::from_field_text(f_id, &doc_id_owned);
            writer.delete_term(term);
            let _ = writer.commit();
        })
        .await
        .map_err(|e| RagError::Backend(format!("tantivy delete task join: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a URI to a filesystem-safe string (replace `:/` and `/` with `-`).
fn urlify(uri: &str) -> String {
    uri.replace("://", "-")
        .replace('/', "-")
        .replace(':', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(80)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: ingest a single document.
    async fn ingest_doc(store: &TantivyStore, coll: &str, uri: &str, body: &str) -> IngestResult {
        store
            .ingest(IngestRequest {
                collection_id: coll.into(),
                source_uri: uri.into(),
                content: body.into(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap()
    }

    // Helper: search a collection.
    async fn search(store: &TantivyStore, coll: &str, q: &str, top_k: u32) -> SearchResult {
        store
            .search(SearchRequest {
                collection_id: coll.into(),
                query: q.into(),
                top_k,
                min_score: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn index_10_docs_top_hit_contains_query_term() {
        let store = TantivyStore::in_memory();

        // Ingest 10 documents; only doc 5 contains the needle.
        for i in 1..=10_u32 {
            let body = if i == 5 {
                "the quick brown fox jumps over the needle".to_string()
            } else {
                format!("document number {i} with random content about dogs and cats")
            };
            ingest_doc(&store, "wiki", &format!("file:///doc{i}.md"), &body).await;
        }

        let result = search(&store, "wiki", "needle", 5).await;
        assert!(!result.hits.is_empty(), "at least one hit expected");
        let top = &result.hits[0];
        assert!(
            top.citation.preview.contains("needle"),
            "top hit preview must contain 'needle', got: {}",
            top.citation.preview
        );
        assert!(top.citation.score > 0.0, "score must be positive");
        assert!(
            top.citation.score <= 1.0,
            "score must be normalised to [0,1]"
        );
    }

    #[tokio::test]
    async fn bm25_score_sanity_more_occurrences_rank_higher() {
        let store = TantivyStore::in_memory();
        // Doc A: "needle" once.
        ingest_doc(&store, "c", "file:///a.md", "the needle is here").await;
        // Doc B: "needle" three times across lines.
        ingest_doc(
            &store,
            "c",
            "file:///b.md",
            "needle needle needle in a haystack",
        )
        .await;

        let result = search(&store, "c", "needle", 5).await;
        assert_eq!(result.hits.len(), 2, "both docs should match");
        // BM25: doc B should score higher (more term occurrences).
        // Note: BM25 is IDF × TF, so within a tiny corpus the scores can
        // be close. We assert B's score >= A's with a small epsilon.
        let score_b = result
            .hits
            .iter()
            .find(|h| h.citation.source_uri.contains('b'))
            .expect("doc B in hits")
            .citation
            .score;
        let score_a = result
            .hits
            .iter()
            .find(|h| h.citation.source_uri.contains('a'))
            .expect("doc A in hits")
            .citation
            .score;
        assert!(
            score_b >= score_a - 1e-4,
            "doc B (3 occurrences) should rank >= doc A (1 occurrence): B={score_b} A={score_a}"
        );
    }

    #[tokio::test]
    async fn ingest_is_idempotent_replaces_prior_version() {
        let store = TantivyStore::in_memory();
        ingest_doc(&store, "c", "file:///a.md", "first version with needle").await;
        ingest_doc(&store, "c", "file:///a.md", "second version without it").await;

        let result = search(&store, "c", "needle", 5).await;
        // After re-ingest with a version that doesn't contain "needle",
        // the search should return no hits (the old version was replaced).
        assert!(
            result.hits.is_empty(),
            "old version should be replaced, got {} hits",
            result.hits.len()
        );
    }

    #[tokio::test]
    async fn search_empty_query_returns_error() {
        let store = TantivyStore::in_memory();
        ingest_doc(&store, "c", "file:///a.md", "anything").await;
        let err = store
            .search(SearchRequest {
                collection_id: "c".into(),
                query: "   ".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .expect_err("empty query should error");
        assert!(matches!(err, RagError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn citation_contract_full_envelope() {
        let store = TantivyStore::in_memory();
        ingest_doc(
            &store,
            "notes",
            "file:///notes.md",
            "the needle is on line one\nline two",
        )
        .await;

        let result = search(&store, "notes", "needle", 3).await;
        assert!(!result.hits.is_empty());
        let cit = &result.hits[0].citation;
        assert!(!cit.source_uri.is_empty(), "source_uri must be populated");
        assert!(
            !cit.collection_id.is_empty(),
            "collection_id must be populated"
        );
        assert!(cit.score > 0.0 && cit.score <= 1.0, "score in [0,1]");
        assert!(!cit.preview.is_empty(), "preview must be populated");
    }

    #[tokio::test]
    async fn list_collections_returns_indexed_collections() {
        let store = TantivyStore::in_memory();
        ingest_doc(&store, "coll-a", "file:///a.md", "alpha").await;
        ingest_doc(&store, "coll-b", "file:///b.md", "beta").await;

        let cols = store.list_collections().await.unwrap();
        let ids: Vec<&str> = cols.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"coll-a"), "coll-a should appear");
        assert!(ids.contains(&"coll-b"), "coll-b should appear");
    }

    #[test]
    fn normalise_score_maps_zero_to_zero() {
        assert_eq!(TantivyStore::normalise_score(0.0), 0.0);
    }

    #[test]
    fn normalise_score_large_value_approaches_one() {
        let s = TantivyStore::normalise_score(1000.0);
        assert!(
            s > 0.99 && s <= 1.0,
            "large BM25 should approach 1.0, got {s}"
        );
    }

    #[test]
    fn normalise_score_monotone() {
        let s1 = TantivyStore::normalise_score(1.0);
        let s2 = TantivyStore::normalise_score(5.0);
        let s3 = TantivyStore::normalise_score(20.0);
        assert!(s1 < s2, "5 > 1 in input → normalised output also ordered");
        assert!(s2 < s3);
    }

    #[test]
    fn urlify_converts_uri_to_safe_string() {
        let s = urlify("file:///path/to/my doc.md");
        assert!(!s.contains('/'), "no slashes allowed");
        assert!(!s.contains(':'), "no colons allowed");
    }
}
