//! `R2RClient` — HTTP adapter to SciPhi-AI's R2R.
//!
//! Production target. R2R runs as a separate service (Docker), so this
//! adapter is purely HTTP — no Python in our binary. The wire shapes
//! mirror R2R v3.x; if R2R bumps to v4 we update the URLs / payload
//! schemas here without touching the rest of xiaoguai.
//!
//! Tested against a real R2R: `cargo test -p xiaoguai-rag --test r2r_e2e -- --ignored`
//! and set `R2R_BASE_URL=http://localhost:7272`. Production parity work
//! (citation line-anchoring, hybrid retrieval defaults) ships as
//! follow-up tickets in v0.9.3+ — for v0.9.2 we land the wiring.

use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

use crate::client::{RagClient, RagError, RagResult};
use crate::types::{
    Citation, Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};

#[derive(Clone)]
pub struct R2RClient {
    base_url: String,
    auth_header: Option<String>,
    http: HttpClient,
}

impl std::fmt::Debug for R2RClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2RClient")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.auth_header.is_some())
            .finish()
    }
}

impl R2RClient {
    /// `base_url` is the R2R service root, e.g. `http://localhost:7272`.
    /// Strip a trailing slash to keep URL composition predictable.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            base_url: base,
            auth_header: None,
            http: HttpClient::new(),
        }
    }

    /// Bearer token / API key, sent verbatim as `Authorization`.
    #[must_use]
    pub fn with_auth(mut self, value: impl Into<String>) -> Self {
        self.auth_header = Some(value.into());
        self
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut b = self.http.request(method, url);
        if let Some(h) = &self.auth_header {
            b = b.header(reqwest::header::AUTHORIZATION, h);
        }
        b
    }
}

fn map_reqwest(e: reqwest::Error) -> RagError {
    RagError::Backend(format!("r2r http: {e}"))
}

fn map_status(status: reqwest::StatusCode, body: &str) -> RagError {
    if status == reqwest::StatusCode::NOT_FOUND {
        return RagError::NotFound(body.to_string());
    }
    RagError::Backend(format!("r2r http {status}: {body}"))
}

#[async_trait]
impl RagClient for R2RClient {
    async fn list_collections(&self) -> RagResult<Vec<Collection>> {
        let resp = self
            .req(reqwest::Method::GET, "/v3/collections")
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        let body = resp.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(map_status(status, &body));
        }
        // R2R returns `{ results: [...] }`. Defend against schema drift
        // by tolerating either a bare array or the wrapped form.
        let v: JsonValue = serde_json::from_str(&body)
            .map_err(|e| RagError::Backend(format!("r2r list_collections parse: {e}")))?;
        let arr = v.get("results").cloned().unwrap_or(v);
        let raw: Vec<R2RCollection> = serde_json::from_value(arr)
            .map_err(|e| RagError::Backend(format!("r2r list_collections schema: {e}")))?;
        Ok(raw.into_iter().map(R2RCollection::into_domain).collect())
    }

    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult> {
        let body = json!({
            "query": req.query,
            "search_settings": {
                "limit": req.top_k,
                "filters": { "collection_id": { "$eq": req.collection_id } },
            }
        });
        let started = std::time::Instant::now();
        let resp = self
            .req(reqwest::Method::POST, "/v3/retrieval/search")
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        let text = resp.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(map_status(status, &text));
        }
        let v: JsonValue = serde_json::from_str(&text)
            .map_err(|e| RagError::Backend(format!("r2r search parse: {e}")))?;
        // R2R envelopes results under `results.chunk_search_results`.
        let chunks = v
            .get("results")
            .and_then(|r| r.get("chunk_search_results"))
            .cloned()
            .unwrap_or(JsonValue::Array(Vec::new()));
        let raw: Vec<R2RChunkHit> = serde_json::from_value(chunks)
            .map_err(|e| RagError::Backend(format!("r2r search schema: {e}")))?;
        let mut hits: Vec<SearchHit> = raw
            .into_iter()
            .map(|h| h.into_hit(&req.collection_id))
            .collect();
        if let Some(min) = req.min_score {
            hits.retain(|h| h.citation.score >= min);
        }
        let elapsed_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        Ok(SearchResult { hits, elapsed_ms })
    }

    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult> {
        let body = json!({
            "collection_ids": [req.collection_id],
            "metadata": req.metadata,
            "ingestion_config": {},
            "raw_text": req.content,
            "source_uri": req.source_uri,
        });
        let resp = self
            .req(reqwest::Method::POST, "/v3/documents")
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        let text = resp.text().await.map_err(map_reqwest)?;
        if !status.is_success() {
            return Err(map_status(status, &text));
        }
        // R2R returns `{ results: { document_id: "...", task_id: "..." } }`.
        // chunk_count is asynchronous in R2R; we don't have it yet.
        let v: JsonValue = serde_json::from_str(&text)
            .map_err(|e| RagError::Backend(format!("r2r ingest parse: {e}")))?;
        let document_id = v
            .get("results")
            .and_then(|r| r.get("document_id"))
            .and_then(|d| d.as_str())
            .ok_or_else(|| RagError::Backend("r2r ingest: no document_id".into()))?
            .to_string();
        Ok(IngestResult {
            document_id,
            chunk_count: 0,
        })
    }

    async fn delete_document(&self, _collection_id: &str, document_id: &str) -> RagResult<()> {
        let path = format!("/v3/documents/{document_id}");
        let resp = self
            .req(reqwest::Method::DELETE, &path)
            .send()
            .await
            .map_err(map_reqwest)?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(()); // idempotent
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_status(status, &body));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct R2RCollection {
    id: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    document_count: u64,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl R2RCollection {
    fn into_domain(self) -> Collection {
        Collection {
            id: self.id,
            name: self.name,
            description: self.description,
            document_count: self.document_count,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct R2RChunkHit {
    #[serde(default)]
    id: Option<String>,
    document_id: String,
    #[serde(default)]
    text: String,
    score: f32,
    #[serde(default)]
    metadata: JsonValue,
}

impl R2RChunkHit {
    fn into_hit(self, collection_id: &str) -> SearchHit {
        // R2R's `metadata.source_uri` is conventional; fall back to a
        // synthetic URI built from the document id. Line span is
        // derived from `metadata.line_start` / `line_end` when set,
        // else `(0, 0)` — backends that can't compute lines must
        // remediate at ingest (citation contract).
        let source_uri = self
            .metadata
            .get("source_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let line_start = self
            .metadata
            .get("line_start")
            .and_then(JsonValue::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let line_end = self
            .metadata
            .get("line_end")
            .and_then(JsonValue::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(line_start);
        let preview = self.text.chars().take(400).collect::<String>();
        SearchHit {
            document_id: self.document_id,
            citation: Citation {
                source_uri: if source_uri.is_empty() {
                    format!("r2r://doc/{}", self.id.unwrap_or_default())
                } else {
                    source_uri
                },
                span: (line_start, line_end),
                score: self.score.clamp(0.0, 1.0),
                preview,
                collection_id: collection_id.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_strips_trailing_slashes() {
        let c = R2RClient::new("http://localhost:7272/");
        assert_eq!(c.base_url, "http://localhost:7272");
        let c = R2RClient::new("http://localhost:7272///");
        assert_eq!(c.base_url, "http://localhost:7272");
    }

    #[test]
    fn r2r_chunk_hit_maps_to_citation_envelope() {
        let raw = serde_json::from_value::<R2RChunkHit>(json!({
            "id": "chunk_1",
            "document_id": "doc_1",
            "text": "found needle here",
            "score": 0.87,
            "metadata": {
                "source_uri": "file:///x.md",
                "line_start": 3,
                "line_end": 5
            }
        }))
        .unwrap();
        let hit = raw.into_hit("notes");
        assert_eq!(hit.citation.source_uri, "file:///x.md");
        assert_eq!(hit.citation.span, (3, 5));
        assert!((hit.citation.score - 0.87).abs() < 1e-5);
        assert_eq!(hit.citation.collection_id, "notes");
        assert!(hit.citation.preview.starts_with("found needle"));
    }

    #[test]
    fn r2r_chunk_hit_without_source_falls_back_to_synthetic() {
        let raw = serde_json::from_value::<R2RChunkHit>(json!({
            "id": "chunk_x",
            "document_id": "doc_y",
            "text": "anonymous",
            "score": 0.4,
            "metadata": {}
        }))
        .unwrap();
        let hit = raw.into_hit("c");
        assert!(hit.citation.source_uri.starts_with("r2r://"));
        assert_eq!(hit.citation.span, (0, 0));
    }
}
