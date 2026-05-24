//! `InMemoryRagClient` — substring-only dev / test backend.
//!
//! Intentionally dumb: stores documents verbatim, splits by line,
//! searches by case-insensitive substring match, scores by hit
//! frequency normalised to `[0, 1]`. Enough to write deterministic
//! eval cases against the citation contract without spinning up R2R.
//!
//! Hard rule: even this toy impl populates the *full* citation
//! envelope (`source_uri + span + score + preview + collection_id`).
//! The citation contract is the load-bearing assertion of v0.9.2 —
//! if it breaks here it breaks everywhere.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;

use crate::client::{RagClient, RagError, RagResult};
use crate::types::{
    Citation, Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};

struct Document {
    id: String,
    source_uri: String,
    /// Line-split content. Each line keeps its 1-indexed line number.
    lines: Vec<String>,
}

#[derive(Default)]
struct CollectionState {
    name: String,
    description: Option<String>,
    created_at: chrono::DateTime<Utc>,
    documents: Vec<Document>,
}

#[derive(Default)]
pub struct InMemoryRagClient {
    state: Mutex<HashMap<String, CollectionState>>,
}

impl std::fmt::Debug for InMemoryRagClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryRagClient")
            .field("collections", &self.state.lock().len())
            .finish()
    }
}

impl InMemoryRagClient {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience for tests: seed a collection with name + optional
    /// description before any docs are ingested.
    pub fn ensure_collection(&self, id: &str, name: &str, description: Option<&str>) {
        let mut g = self.state.lock();
        g.entry(id.to_string()).or_insert_with(|| CollectionState {
            name: name.into(),
            description: description.map(str::to_string),
            created_at: Utc::now(),
            documents: Vec::new(),
        });
    }
}

#[async_trait]
impl RagClient for InMemoryRagClient {
    async fn list_collections(&self) -> RagResult<Vec<Collection>> {
        let g = self.state.lock();
        let mut out: Vec<Collection> = g
            .iter()
            .map(|(id, c)| Collection {
                id: id.clone(),
                name: c.name.clone(),
                description: c.description.clone(),
                document_count: u64::try_from(c.documents.len()).unwrap_or(u64::MAX),
                created_at: c.created_at,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult> {
        let start = std::time::Instant::now();
        let q = req.query.trim();
        if q.is_empty() {
            return Err(RagError::InvalidArgument("query is empty".into()));
        }
        let needle = q.to_ascii_lowercase();

        let g = self.state.lock();
        let coll = g
            .get(&req.collection_id)
            .ok_or_else(|| RagError::NotFound(req.collection_id.clone()))?;

        // Score = (matching lines / total lines) for the document,
        // clamped to [0, 1]. Crude but deterministic.
        let mut hits: Vec<SearchHit> = Vec::new();
        for doc in &coll.documents {
            let mut matched: Vec<(u32, &str)> = Vec::new();
            for (idx, line) in doc.lines.iter().enumerate() {
                if line.to_ascii_lowercase().contains(&needle) {
                    // Lines are 1-indexed in the citation API.
                    let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
                    matched.push((line_no, line.as_str()));
                }
            }
            if matched.is_empty() {
                continue;
            }
            let total = doc.lines.len().max(1);
            #[allow(clippy::cast_precision_loss)]
            let score = (matched.len() as f32 / total as f32).min(1.0);
            if let Some(min) = req.min_score {
                if score < min {
                    continue;
                }
            }
            // Use the first matched line as the citation anchor.
            let (line_no, line_text) = matched[0];
            hits.push(SearchHit {
                document_id: doc.id.clone(),
                citation: Citation {
                    source_uri: doc.source_uri.clone(),
                    span: (line_no, line_no),
                    score,
                    preview: line_text.to_string(),
                    collection_id: req.collection_id.clone(),
                },
            });
        }
        hits.sort_by(|a, b| {
            b.citation
                .score
                .partial_cmp(&a.citation.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_k = usize::try_from(req.top_k).unwrap_or(usize::MAX);
        hits.truncate(top_k);

        let elapsed_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
        Ok(SearchResult { hits, elapsed_ms })
    }

    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult> {
        let mut g = self.state.lock();
        let coll = g
            .entry(req.collection_id.clone())
            .or_insert_with(|| CollectionState {
                name: req.collection_id.clone(),
                description: None,
                created_at: Utc::now(),
                documents: Vec::new(),
            });

        // Idempotent on source_uri: replace prior version.
        coll.documents.retain(|d| d.source_uri != req.source_uri);

        let lines: Vec<String> = req.content.lines().map(str::to_string).collect();
        let chunk_count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let document_id = format!("doc_{}", coll.documents.len() + 1);
        coll.documents.push(Document {
            id: document_id.clone(),
            source_uri: req.source_uri,
            lines,
        });
        Ok(IngestResult {
            document_id,
            chunk_count,
        })
    }

    async fn delete_document(&self, collection_id: &str, document_id: &str) -> RagResult<()> {
        let mut g = self.state.lock();
        if let Some(coll) = g.get_mut(collection_id) {
            coll.documents.retain(|d| d.id != document_id);
        }
        Ok(())
    }

    async fn reindex_path(&self, collection_id: &str, path: &std::path::Path) -> RagResult<usize> {
        // Read off the lock — file IO can block; we don't want it under
        // the mutex.
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| RagError::InvalidArgument(format!("read {}: {e}", path.display())))?;
        let source_uri = format!("file://{}", path.display());
        let ingest_req = IngestRequest {
            collection_id: collection_id.into(),
            source_uri,
            content,
            metadata: serde_json::json!({ "reindexed_at": Utc::now().to_rfc3339() }),
        };
        let outcome = self.ingest(ingest_req).await?;
        Ok(usize::try_from(outcome.chunk_count).unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ingest(c: &InMemoryRagClient, coll: &str, uri: &str, body: &str) -> IngestResult {
        c.ingest(IngestRequest {
            collection_id: coll.into(),
            source_uri: uri.into(),
            content: body.into(),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn search_returns_citation_with_full_envelope() {
        let c = InMemoryRagClient::new();
        c.ensure_collection("notes", "Notes", None);
        c.ingest(IngestRequest {
            collection_id: "notes".into(),
            source_uri: "file:///x/notes.md".into(),
            content: "first line\nthe needle is here\nthird line".into(),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

        let res = c
            .search(SearchRequest {
                collection_id: "notes".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .unwrap();
        assert_eq!(res.hits.len(), 1);
        let cit = &res.hits[0].citation;
        assert_eq!(cit.source_uri, "file:///x/notes.md");
        assert_eq!(cit.span, (2, 2));
        assert!(cit.score > 0.0 && cit.score <= 1.0);
        assert_eq!(cit.collection_id, "notes");
        assert!(cit.preview.contains("needle"));
    }

    #[tokio::test]
    async fn ingest_is_idempotent_on_source_uri() {
        let c = InMemoryRagClient::new();
        ingest(&c, "x", "file:///a.md", "v1").await;
        ingest(&c, "x", "file:///a.md", "v2 has needle").await;

        let res = c
            .search(SearchRequest {
                collection_id: "x".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .unwrap();
        assert_eq!(res.hits.len(), 1);
        assert!(res.hits[0].citation.preview.contains("needle"));
    }

    #[tokio::test]
    async fn search_unknown_collection_returns_not_found() {
        let c = InMemoryRagClient::new();
        let err = c
            .search(SearchRequest {
                collection_id: "missing".into(),
                query: "x".into(),
                top_k: 1,
                min_score: None,
            })
            .await
            .expect_err("should be NotFound");
        assert!(matches!(err, RagError::NotFound(_)));
    }

    #[tokio::test]
    async fn reindex_path_reads_file_and_returns_chunk_count() {
        let c = InMemoryRagClient::new();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("note.md");
        tokio::fs::write(&p, "line one\nline two\nthe needle is here\n")
            .await
            .unwrap();

        let n = c.reindex_path("notes", &p).await.unwrap();
        assert_eq!(n, 3, "three lines = three chunks");

        let res = c
            .search(SearchRequest {
                collection_id: "notes".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .unwrap();
        assert_eq!(res.hits.len(), 1);
        assert!(res.hits[0].citation.source_uri.starts_with("file://"));
        assert!(res.hits[0].citation.source_uri.ends_with("note.md"));
    }

    #[tokio::test]
    async fn reindex_path_is_idempotent_replacing_prior_content() {
        let c = InMemoryRagClient::new();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("note.md");

        tokio::fs::write(&p, "first version with needle\n")
            .await
            .unwrap();
        c.reindex_path("notes", &p).await.unwrap();

        tokio::fs::write(&p, "second version without it\n")
            .await
            .unwrap();
        c.reindex_path("notes", &p).await.unwrap();

        let res = c
            .search(SearchRequest {
                collection_id: "notes".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .unwrap();
        assert!(
            res.hits.is_empty(),
            "second reindex must replace, not append"
        );
    }

    #[tokio::test]
    async fn reindex_path_errors_when_file_missing() {
        let c = InMemoryRagClient::new();
        let err = c
            .reindex_path("notes", std::path::Path::new("/tmp/does-not-exist-xyz.md"))
            .await
            .expect_err("missing file should error");
        assert!(matches!(err, RagError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn min_score_filter_drops_low_hits() {
        let c = InMemoryRagClient::new();
        // Three-line doc; one match → score = 1/3 ≈ 0.33.
        c.ingest(IngestRequest {
            collection_id: "x".into(),
            source_uri: "file:///a.md".into(),
            content: "a\nneedle\nc".into(),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
        let res = c
            .search(SearchRequest {
                collection_id: "x".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: Some(0.5),
            })
            .await
            .unwrap();
        assert!(res.hits.is_empty());
    }
}
