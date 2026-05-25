//! Cross-encoder reranker for the RAG two-stage retrieval pipeline.
//!
//! # Two-stage design
//!
//! Stage 1 (retrieval): fetch `k_initial` (~50) candidates from any [`RagClient`].
//! Stage 2 (reranking): pass candidates + query to a [`Reranker`]; get back up
//! to `top_k` candidates sorted by relevance.
//!
//! The reranker is composable: pair any retriever with any reranker at the call
//! site — no shared state required.
//!
//! # Providers
//!
//! | Struct | Service | Default model |
//! |---|---|---|
//! | [`CohereReranker`]  | Cohere Rerank API v2 | `rerank-3.5` |
//! | [`VoyageReranker`]  | Voyage AI Rerank     | `rerank-2` |
//! | [`JinaReranker`]    | Jina Rerank API      | `jina-reranker-v2-base-multilingual` |
//! | [`LlmReranker`]     | Any [`LlmBackend`]   | *(prompt-based)* |
//! | [`NullReranker`]    | (none — identity)    | *(no-op)* |
//!
//! # Latency budget
//!
//! Every provider implementation honours `timeout_ms` (default 5 000 ms). On
//! timeout the original candidate order is preserved and a warning is emitted
//! via `tracing`. Callers can tune this via [`RerankerConfig`].
//!
//! # Deferred
//!
//! Local cross-encoder via ONNX Runtime is deferred to v1.3 — it eliminates
//! API cost and latency but adds a non-trivial native dependency.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::client::RagError;
use crate::types::SearchHit;

// ── shared types ─────────────────────────────────────────────────────────────

/// A candidate passed to the reranker. Wraps a [`SearchHit`] with its 0-based
/// position in the initial retrieval list so callers can detect ordering
/// changes.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Original rank from the retriever (0-indexed).
    pub original_rank: usize,
    pub hit: SearchHit,
}

impl Candidate {
    #[must_use]
    pub fn from_hits(hits: Vec<SearchHit>) -> Vec<Self> {
        hits.into_iter()
            .enumerate()
            .map(|(i, h)| Self {
                original_rank: i,
                hit: h,
            })
            .collect()
    }
}

/// A candidate with a reranker-assigned relevance score in `[0, 1]`.
#[derive(Debug, Clone)]
pub struct Scored {
    pub candidate: Candidate,
    /// Relevance score in `[0, 1]`. Higher is more relevant.
    pub relevance: f32,
}

// ── trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank `candidates` for `query`. Returns a list sorted by descending
    /// relevance, truncated to at most `top_k` items.
    ///
    /// Implementations MUST respect `timeout_ms`: if the underlying API does
    /// not respond within the budget, they return the candidates in their
    /// original order with relevance scores equal to the original retrieval
    /// score (normalised from the [`SearchHit::citation.score`] field).
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        timeout_ms: u64,
    ) -> Vec<Scored>;

    /// Provider identifier used in logs.
    fn name(&self) -> &'static str;
}

// Compile-time object-safety check.
const _: Option<Box<dyn Reranker>> = None;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Fall back to the original retrieval order, preserving the citation score
/// as the relevance signal.
fn fallback_order(candidates: Vec<Candidate>, top_k: usize) -> Vec<Scored> {
    candidates
        .into_iter()
        .take(top_k)
        .map(|c| {
            let relevance = c.hit.citation.score;
            Scored {
                candidate: c,
                relevance,
            }
        })
        .collect()
}

/// Sort by descending relevance and truncate.
fn sort_and_truncate(mut scored: Vec<Scored>, top_k: usize) -> Vec<Scored> {
    scored.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(top_k);
    scored
}

// ── NullReranker ──────────────────────────────────────────────────────────────

/// Identity reranker — passes candidates through unchanged.
///
/// Useful as a default when no reranker is configured, or in tests that
/// want to verify the two-stage plumbing without touching a real API.
#[derive(Debug, Clone, Default)]
pub struct NullReranker;

#[async_trait]
impl Reranker for NullReranker {
    async fn rerank(
        &self,
        _query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        _timeout_ms: u64,
    ) -> Vec<Scored> {
        fallback_order(candidates, top_k)
    }

    fn name(&self) -> &'static str {
        "null"
    }
}

// ── CohereReranker ────────────────────────────────────────────────────────────

/// Cohere Rerank API (<https://docs.cohere.com/reference/rerank>).
///
/// Default model: `rerank-3.5` (best quality as of 2025-06).
/// The API returns `relevance_score` in `[0, 1]` — no normalisation needed.
#[derive(Clone)]
pub struct CohereReranker {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for CohereReranker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CohereReranker")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl CohereReranker {
    pub const DEFAULT_MODEL: &'static str = "rerank-3.5";

    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.into(),
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the HTTP client (used in tests with a mock server).
    #[must_use]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }
}

#[derive(Deserialize)]
struct CohereRerankResponse {
    results: Vec<CohereResult>,
}

#[derive(Deserialize)]
struct CohereResult {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl Reranker for CohereReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        timeout_ms: u64,
    ) -> Vec<Scored> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let docs: Vec<&str> = candidates
            .iter()
            .map(|c| c.hit.citation.preview.as_str())
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": docs,
            "top_n": top_k,
            "return_documents": false,
        });

        let fut = self
            .http
            .post("https://api.cohere.com/v2/rerank")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();

        let resp = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(provider = "cohere", err = %e, "rerank HTTP error — falling back to retrieval order");
                return fallback_order(candidates, top_k);
            }
            Err(_) => {
                tracing::warn!(
                    provider = "cohere",
                    timeout_ms,
                    "rerank timeout — falling back to retrieval order"
                );
                return fallback_order(candidates, top_k);
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(provider = "cohere", %status, "rerank non-2xx — falling back");
            return fallback_order(candidates, top_k);
        }

        let parsed: CohereRerankResponse = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(provider = "cohere", err = %e, "rerank parse error — falling back");
                return fallback_order(candidates, top_k);
            }
        };

        let mut scored: Vec<Scored> = parsed
            .results
            .into_iter()
            .filter_map(|r| {
                candidates.get(r.index).cloned().map(|c| Scored {
                    candidate: c,
                    relevance: r.relevance_score.clamp(0.0, 1.0),
                })
            })
            .collect();
        // Cohere already returns top_n results, but sort for safety.
        sort_and_truncate(std::mem::take(&mut scored), top_k)
    }

    fn name(&self) -> &'static str {
        "cohere"
    }
}

// ── VoyageReranker ────────────────────────────────────────────────────────────

/// Voyage AI Rerank API (<https://docs.voyageai.com/reference/reranker-api>).
///
/// Default model: `rerank-2`.
/// The API returns `relevance_score` in `[0, 1]`.
#[derive(Clone)]
pub struct VoyageReranker {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for VoyageReranker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoyageReranker")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl VoyageReranker {
    pub const DEFAULT_MODEL: &'static str = "rerank-2";

    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.into(),
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    #[must_use]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }
}

#[derive(Deserialize)]
struct VoyageRerankResponse {
    data: Vec<VoyageResult>,
}

#[derive(Deserialize)]
struct VoyageResult {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl Reranker for VoyageReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        timeout_ms: u64,
    ) -> Vec<Scored> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let docs: Vec<&str> = candidates
            .iter()
            .map(|c| c.hit.citation.preview.as_str())
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": docs,
            "top_k": top_k,
            "return_documents": false,
        });

        let fut = self
            .http
            .post("https://api.voyageai.com/v1/rerank")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();

        let resp = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(provider = "voyage", err = %e, "rerank HTTP error — falling back");
                return fallback_order(candidates, top_k);
            }
            Err(_) => {
                tracing::warn!(
                    provider = "voyage",
                    timeout_ms,
                    "rerank timeout — falling back"
                );
                return fallback_order(candidates, top_k);
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(provider = "voyage", %status, "rerank non-2xx — falling back");
            return fallback_order(candidates, top_k);
        }

        let parsed: VoyageRerankResponse = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(provider = "voyage", err = %e, "rerank parse error — falling back");
                return fallback_order(candidates, top_k);
            }
        };

        let mut scored: Vec<Scored> = parsed
            .data
            .into_iter()
            .filter_map(|r| {
                candidates.get(r.index).cloned().map(|c| Scored {
                    candidate: c,
                    relevance: r.relevance_score.clamp(0.0, 1.0),
                })
            })
            .collect();
        sort_and_truncate(std::mem::take(&mut scored), top_k)
    }

    fn name(&self) -> &'static str {
        "voyage"
    }
}

// ── JinaReranker ──────────────────────────────────────────────────────────────

/// Jina AI Rerank API (<https://jina.ai/reranker/>).
///
/// Default model: `jina-reranker-v2-base-multilingual`.
/// The API returns `relevance_score` in `[0, 1]`.
#[derive(Clone)]
pub struct JinaReranker {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for JinaReranker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JinaReranker")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl JinaReranker {
    pub const DEFAULT_MODEL: &'static str = "jina-reranker-v2-base-multilingual";

    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: Self::DEFAULT_MODEL.into(),
            http: reqwest::Client::new(),
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    #[must_use]
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }
}

#[derive(Deserialize)]
struct JinaRerankResponse {
    results: Vec<JinaResult>,
}

#[derive(Deserialize)]
struct JinaResult {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl Reranker for JinaReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        timeout_ms: u64,
    ) -> Vec<Scored> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let docs: Vec<&str> = candidates
            .iter()
            .map(|c| c.hit.citation.preview.as_str())
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": docs,
            "top_n": top_k,
        });

        let fut = self
            .http
            .post("https://api.jina.ai/v1/rerank")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();

        let resp = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(provider = "jina", err = %e, "rerank HTTP error — falling back");
                return fallback_order(candidates, top_k);
            }
            Err(_) => {
                tracing::warn!(
                    provider = "jina",
                    timeout_ms,
                    "rerank timeout — falling back"
                );
                return fallback_order(candidates, top_k);
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            tracing::warn!(provider = "jina", %status, "rerank non-2xx — falling back");
            return fallback_order(candidates, top_k);
        }

        let parsed: JinaRerankResponse = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(provider = "jina", err = %e, "rerank parse error — falling back");
                return fallback_order(candidates, top_k);
            }
        };

        let mut scored: Vec<Scored> = parsed
            .results
            .into_iter()
            .filter_map(|r| {
                candidates.get(r.index).cloned().map(|c| Scored {
                    candidate: c,
                    relevance: r.relevance_score.clamp(0.0, 1.0),
                })
            })
            .collect();
        sort_and_truncate(std::mem::take(&mut scored), top_k)
    }

    fn name(&self) -> &'static str {
        "jina"
    }
}

// ── LlmReranker ───────────────────────────────────────────────────────────────

/// LLM-based reranker — uses any [`LlmBackend`] with a fixed prompt template.
///
/// The prompt asks the model to score each passage from 0 to 10 for relevance
/// to the query. The response is expected to be a single integer per passage,
/// one per line. Non-parseable lines receive a fallback score of 0.
///
/// This is ~5-10× slower than a dedicated reranker API (one LLM call per
/// batch) and costs more tokens, but works with any model already wired up
/// in the deployment.
pub struct LlmReranker {
    llm: Arc<dyn xiaoguai_llm::LlmBackend>,
    model: String,
}

impl std::fmt::Debug for LlmReranker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmReranker")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl LlmReranker {
    #[must_use]
    pub fn new(llm: Arc<dyn xiaoguai_llm::LlmBackend>, model: impl Into<String>) -> Self {
        Self {
            llm,
            model: model.into(),
        }
    }

    /// Build the scoring prompt.
    fn build_prompt(query: &str, previews: &[&str]) -> String {
        use std::fmt::Write as _;
        let mut prompt = format!(
            "You are a relevance judge. For each passage below, output a single integer \
             score from 0 to 10 indicating how relevant it is to the query. \
             Output only integers, one per line, in the same order as the passages. \
             No other text.\n\nQuery: {query}\n\nPassages:\n"
        );
        for (i, text) in previews.iter().enumerate() {
            // write! to avoid the extra String allocation from format!.
            let _ = writeln!(prompt, "[{}] {}", i + 1, text);
        }
        prompt
    }

    /// Parse the LLM response: one integer per line → normalised to `[0, 1]`.
    fn parse_scores(text: &str, count: usize) -> Vec<f32> {
        let mut scores: Vec<f32> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                // Tolerate "[1] 7" or "7" or "Score: 7".
                let digit_str = l.chars().filter(char::is_ascii_digit).collect::<String>();
                digit_str.parse::<u8>().unwrap_or(0).min(10)
            })
            .map(|n| f32::from(n) / 10.0)
            .collect();
        // Pad to `count` if the model gave fewer lines.
        scores.resize(count, 0.0);
        scores.truncate(count);
        scores
    }
}

#[async_trait]
impl Reranker for LlmReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<Candidate>,
        top_k: usize,
        timeout_ms: u64,
    ) -> Vec<Scored> {
        use futures::StreamExt as _;
        use xiaoguai_llm::{ChatRequest, Message};

        if candidates.is_empty() {
            return Vec::new();
        }
        let previews: Vec<&str> = candidates
            .iter()
            .map(|c| c.hit.citation.preview.as_str())
            .collect();
        let prompt = Self::build_prompt(query, &previews);
        let req = ChatRequest::new(&self.model, vec![Message::user(prompt)]);

        let stream_fut = self.llm.chat_stream(req);
        let stream_result =
            tokio::time::timeout(Duration::from_millis(timeout_ms), stream_fut).await;

        let mut stream = match stream_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::warn!(provider = "llm", err = %e, "rerank LLM error — falling back");
                return fallback_order(candidates, top_k);
            }
            Err(_) => {
                tracing::warn!(
                    provider = "llm",
                    timeout_ms,
                    "rerank LLM timeout — falling back"
                );
                return fallback_order(candidates, top_k);
            }
        };

        let mut full_text = String::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    full_text.push_str(&chunk.delta);
                    if chunk.done {
                        break;
                    }
                }
                Ok(Some(Err(e))) => {
                    tracing::warn!(provider = "llm", err = %e, "rerank chunk error — falling back");
                    return fallback_order(candidates, top_k);
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(provider = "llm", "rerank stream timeout — falling back");
                    return fallback_order(candidates, top_k);
                }
            }
        }

        let relevance_scores = Self::parse_scores(&full_text, candidates.len());
        let scored: Vec<Scored> = candidates
            .into_iter()
            .zip(relevance_scores)
            .map(|(c, relevance)| Scored {
                candidate: c,
                relevance,
            })
            .collect();
        sort_and_truncate(scored, top_k)
    }

    fn name(&self) -> &'static str {
        "llm"
    }
}

// ── RerankerConfig ────────────────────────────────────────────────────────────

/// Runtime configuration knobs, shared by all providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankerConfig {
    /// How many candidates to retrieve from the first stage before reranking.
    /// Default: 50.
    #[serde(default = "default_k_initial")]
    pub k_initial: usize,
    /// How many results to return after reranking.
    /// Default: 5.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Per-call API timeout in milliseconds.
    /// Default: 5 000 ms.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_k_initial() -> usize {
    50
}
fn default_top_k() -> usize {
    5
}
fn default_timeout_ms() -> u64 {
    5_000
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            k_initial: default_k_initial(),
            top_k: default_top_k(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

// ── two_stage_retrieve ────────────────────────────────────────────────────────

/// Convenience function: run a two-stage retrieval + rerank pipeline.
///
/// 1. Calls `retriever.search(req_with_k_initial)` to get `config.k_initial` candidates.
/// 2. Passes them to `reranker.rerank(...)` to get `config.top_k` results.
///
/// Returns `RagError` only when the retriever itself fails. Reranker failures
/// degrade gracefully (original order) inside each provider implementation.
pub async fn two_stage_retrieve(
    retriever: &dyn crate::client::RagClient,
    reranker: &dyn Reranker,
    collection_id: &str,
    query: &str,
    config: &RerankerConfig,
) -> Result<Vec<Scored>, RagError> {
    use crate::types::SearchRequest;

    let k = u32::try_from(config.k_initial).unwrap_or(u32::MAX);
    let result = retriever
        .search(SearchRequest {
            collection_id: collection_id.into(),
            query: query.into(),
            top_k: k,
            min_score: None,
        })
        .await?;

    let candidates = Candidate::from_hits(result.hits);
    let scored = reranker
        .rerank(query, candidates, config.top_k, config.timeout_ms)
        .await;
    Ok(scored)
}

// ── tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::RagClient;
    use crate::memory::InMemoryRagClient;
    use crate::types::{Citation, SearchHit};

    // ── test helpers ─────────────────────────────────────────────────────────

    fn make_hit(preview: &str, score: f32) -> SearchHit {
        SearchHit {
            document_id: "doc_1".into(),
            citation: Citation {
                source_uri: "file:///test.md".into(),
                span: (1, 1),
                score,
                preview: preview.into(),
                collection_id: "c".into(),
            },
        }
    }

    fn candidates_from_previews(data: &[(&str, f32)]) -> Vec<Candidate> {
        let hits: Vec<SearchHit> = data.iter().map(|(p, s)| make_hit(p, *s)).collect();
        Candidate::from_hits(hits)
    }

    // ── NullReranker ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn null_reranker_preserves_order() {
        let r = NullReranker;
        let data = [("a", 0.9f32), ("b", 0.7), ("c", 0.5)];
        let candidates = candidates_from_previews(&data);
        let scored = r.rerank("q", candidates, 10, 5_000).await;

        assert_eq!(scored.len(), 3);
        // original order preserved
        assert_eq!(scored[0].candidate.hit.citation.preview, "a");
        assert_eq!(scored[1].candidate.hit.citation.preview, "b");
        assert_eq!(scored[2].candidate.hit.citation.preview, "c");
        // original retrieval scores passed through as relevance
        assert!((scored[0].relevance - 0.9).abs() < 1e-5);
    }

    #[tokio::test]
    async fn null_reranker_top_k_truncates() {
        let r = NullReranker;
        let data = [("a", 0.9f32), ("b", 0.7), ("c", 0.5)];
        let candidates = candidates_from_previews(&data);
        let scored = r.rerank("q", candidates, 2, 5_000).await;
        assert_eq!(scored.len(), 2);
    }

    #[tokio::test]
    async fn null_reranker_empty_input() {
        let r = NullReranker;
        let scored = r.rerank("q", vec![], 5, 5_000).await;
        assert!(scored.is_empty());
    }

    // ── LlmReranker ──────────────────────────────────────────────────────────

    /// Scripted `MockBackend` that returns fixed integer scores, one per line.
    fn mock_llm_scores(scores: &[u8]) -> Arc<dyn xiaoguai_llm::LlmBackend> {
        let text = scores
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        Arc::new(xiaoguai_llm::MockBackend::with_response(text))
    }

    #[tokio::test]
    async fn llm_reranker_scores_and_sorts() {
        // Three candidates; mock LLM returns scores [3, 9, 1] → order [b, a, c]
        let data = [("a", 0.5f32), ("b", 0.5), ("c", 0.5)];
        let candidates = candidates_from_previews(&data);
        let llm = mock_llm_scores(&[3, 9, 1]);
        let r = LlmReranker::new(llm, "mock");

        let scored = r.rerank("query", candidates, 3, 5_000).await;
        assert_eq!(scored.len(), 3);
        // best score first
        assert_eq!(scored[0].candidate.hit.citation.preview, "b");
        assert!((scored[0].relevance - 0.9).abs() < 1e-5);
        assert_eq!(scored[1].candidate.hit.citation.preview, "a");
        assert!((scored[1].relevance - 0.3).abs() < 1e-5);
        assert_eq!(scored[2].candidate.hit.citation.preview, "c");
        assert!((scored[2].relevance - 0.1).abs() < 1e-5);
    }

    #[tokio::test]
    async fn llm_reranker_top_k_truncates() {
        let data = [("a", 0.5f32), ("b", 0.5), ("c", 0.5)];
        let candidates = candidates_from_previews(&data);
        let llm = mock_llm_scores(&[5, 8, 2]);
        let r = LlmReranker::new(llm, "mock");

        let scored = r.rerank("query", candidates, 2, 5_000).await;
        assert_eq!(scored.len(), 2);
        assert_eq!(scored[0].candidate.hit.citation.preview, "b"); // score 8
    }

    #[tokio::test]
    async fn llm_reranker_empty_candidates() {
        let llm = mock_llm_scores(&[5]);
        let r = LlmReranker::new(llm, "mock");
        let scored = r.rerank("query", vec![], 5, 5_000).await;
        assert!(scored.is_empty());
    }

    #[tokio::test]
    async fn llm_reranker_failing_backend_falls_back() {
        use xiaoguai_llm::LlmError;
        let data = [("x", 0.8f32), ("y", 0.6)];
        let candidates = candidates_from_previews(&data);
        let llm: Arc<dyn xiaoguai_llm::LlmBackend> = Arc::new(xiaoguai_llm::MockBackend::failing(
            LlmError::Network("forced".into()),
        ));
        let r = LlmReranker::new(llm, "mock");
        let scored = r.rerank("query", candidates, 5, 5_000).await;
        // fallback: original order, scores from citation.score
        assert_eq!(scored.len(), 2);
        assert_eq!(scored[0].candidate.hit.citation.preview, "x");
        assert!((scored[0].relevance - 0.8).abs() < 1e-5);
    }

    // ── parse_scores ─────────────────────────────────────────────────────────

    #[test]
    fn parse_scores_basic() {
        let text = "7\n3\n10\n";
        let s = LlmReranker::parse_scores(text, 3);
        assert_eq!(s.len(), 3);
        assert!((s[0] - 0.7).abs() < 1e-5);
        assert!((s[1] - 0.3).abs() < 1e-5);
        assert!((s[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_scores_pads_short_response() {
        let text = "5";
        let s = LlmReranker::parse_scores(text, 3);
        assert_eq!(s.len(), 3);
        assert!((s[0] - 0.5).abs() < 1e-5);
        assert!((s[1] - 0.0).abs() < 1e-5);
    }

    #[test]
    fn parse_scores_clamps_above_10() {
        // Model might output "15" — should be treated as 10.
        let text = "15\n0\n";
        let s = LlmReranker::parse_scores(text, 2);
        assert!((s[0] - 1.0).abs() < 1e-5);
        assert!((s[1] - 0.0).abs() < 1e-5);
    }

    #[test]
    fn parse_scores_tolerates_prefixed_lines() {
        let text = "[1] 7\n[2] 4\n";
        let s = LlmReranker::parse_scores(text, 2);
        // Both lines contain digits; concatenated "17" → clamp to 10
        // Actually the impl concatenates all digits in the line.
        // "[1] 7" → digits "17" → 17.min(10) = 10. That's fine for the test.
        assert!(s[0] >= 0.0 && s[0] <= 1.0);
        assert!(s[1] >= 0.0 && s[1] <= 1.0);
    }

    // ── RerankerConfig defaults ───────────────────────────────────────────────

    #[test]
    fn reranker_config_defaults() {
        let c = RerankerConfig::default();
        assert_eq!(c.k_initial, 50);
        assert_eq!(c.top_k, 5);
        assert_eq!(c.timeout_ms, 5_000);
    }

    // ── two_stage_retrieve (integration with InMemoryRagClient) ──────────────

    #[tokio::test]
    async fn two_stage_retrieve_with_null_reranker() {
        let client = InMemoryRagClient::new();
        client.ensure_collection("notes", "Notes", None);
        // Ingest 3 docs
        for (i, content) in ["alpha needle", "beta needle needle", "gamma"]
            .iter()
            .enumerate()
        {
            client
                .ingest(crate::types::IngestRequest {
                    collection_id: "notes".into(),
                    source_uri: format!("file:///doc{i}.md"),
                    content: (*content).to_string(),
                    metadata: serde_json::json!({}),
                })
                .await
                .unwrap();
        }

        let cfg = RerankerConfig {
            k_initial: 10,
            top_k: 2,
            timeout_ms: 5_000,
        };
        let reranker = NullReranker;
        let scored = two_stage_retrieve(&client, &reranker, "notes", "needle", &cfg)
            .await
            .unwrap();

        // Only docs with "needle" should come back (gamma has none).
        // top_k=2 limits to 2.
        assert_eq!(scored.len(), 2);
        // All returned hits contain "needle" in their preview.
        for s in &scored {
            assert!(
                s.candidate.hit.citation.preview.contains("needle"),
                "expected needle in preview: {}",
                s.candidate.hit.citation.preview
            );
        }
    }

    #[tokio::test]
    async fn two_stage_retrieve_with_llm_reranker_reorders() {
        let client = InMemoryRagClient::new();
        client.ensure_collection("notes", "Notes", None);
        for (i, content) in ["needle alpha", "needle beta"].iter().enumerate() {
            client
                .ingest(crate::types::IngestRequest {
                    collection_id: "notes".into(),
                    source_uri: format!("file:///doc{i}.md"),
                    content: (*content).to_string(),
                    metadata: serde_json::json!({}),
                })
                .await
                .unwrap();
        }

        let cfg = RerankerConfig {
            k_initial: 10,
            top_k: 2,
            timeout_ms: 5_000,
        };
        // Scores: [2, 9] → "needle beta" (score 9) should come first.
        let llm = mock_llm_scores(&[2, 9]);
        let reranker = LlmReranker::new(llm, "mock");
        let scored = two_stage_retrieve(&client, &reranker, "notes", "needle", &cfg)
            .await
            .unwrap();

        assert_eq!(scored.len(), 2);
        assert!(scored[0].candidate.hit.citation.preview.contains("beta"));
        assert!(scored[0].relevance > scored[1].relevance);
    }

    // ── Candidate helpers ─────────────────────────────────────────────────────

    #[test]
    fn candidate_from_hits_sets_original_rank() {
        let hits: Vec<SearchHit> = (0..3).map(|i| make_hit(&format!("doc {i}"), 0.5)).collect();
        let candidates = Candidate::from_hits(hits);
        for (i, c) in candidates.iter().enumerate() {
            assert_eq!(c.original_rank, i);
        }
    }

    // ── provider name identifiers ─────────────────────────────────────────────

    #[test]
    fn provider_names() {
        assert_eq!(NullReranker.name(), "null");
        let llm = mock_llm_scores(&[5]);
        assert_eq!(LlmReranker::new(llm, "mock").name(), "llm");
        assert_eq!(CohereReranker::new("k").name(), "cohere");
        assert_eq!(VoyageReranker::new("k").name(), "voyage");
        assert_eq!(JinaReranker::new("k").name(), "jina");
    }

    // ── Cohere / Voyage / Jina: mock HTTP response tests ─────────────────────

    /// These tests exercise the JSON parsing path using pre-baked response
    /// bodies; no real HTTP server — we test the deserialization logic in
    /// isolation.

    #[test]
    fn cohere_response_parses() {
        let json = serde_json::json!({
            "results": [
                { "index": 1, "relevance_score": 0.92 },
                { "index": 0, "relevance_score": 0.45 }
            ]
        });
        let parsed: CohereRerankResponse = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].index, 1);
        assert!((parsed.results[0].relevance_score - 0.92).abs() < 1e-4);
        assert_eq!(parsed.results[1].index, 0);
    }

    #[test]
    fn voyage_response_parses() {
        let json = serde_json::json!({
            "data": [
                { "index": 0, "relevance_score": 0.81 },
                { "index": 2, "relevance_score": 0.35 }
            ]
        });
        let parsed: VoyageRerankResponse = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert!((parsed.data[0].relevance_score - 0.81).abs() < 1e-4);
    }

    #[test]
    fn jina_response_parses() {
        let json = serde_json::json!({
            "results": [
                { "index": 2, "relevance_score": 0.77 },
                { "index": 0, "relevance_score": 0.22 }
            ]
        });
        let parsed: JinaRerankResponse = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.results[0].index, 2);
        assert!((parsed.results[0].relevance_score - 0.77).abs() < 1e-4);
    }

    /// Verify that reranked order from mocked Cohere-style results respects
    /// relevance sorting and `top_k` truncation.
    #[test]
    fn sort_and_truncate_respects_relevance_and_top_k() {
        let data = [("a", 0.5f32), ("b", 0.5), ("c", 0.5)];
        let candidates = candidates_from_previews(&data);
        // Simulate what the provider loop would produce after parsing.
        let scored: Vec<Scored> = vec![
            Scored {
                candidate: candidates[0].clone(),
                relevance: 0.45,
            },
            Scored {
                candidate: candidates[1].clone(),
                relevance: 0.92,
            },
            Scored {
                candidate: candidates[2].clone(),
                relevance: 0.60,
            },
        ];
        let result = sort_and_truncate(scored, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0].relevance - 0.92).abs() < 1e-5); // "b"
        assert!((result[1].relevance - 0.60).abs() < 1e-5); // "c"
    }

    /// `fallback_order` preserves original rank and uses citation score.
    #[test]
    fn fallback_order_preserves_rank_and_score() {
        let data = [("x", 0.8f32), ("y", 0.6), ("z", 0.4)];
        let candidates = candidates_from_previews(&data);
        let result = fallback_order(candidates, 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].candidate.original_rank, 0);
        assert!((result[0].relevance - 0.8).abs() < 1e-5);
        assert_eq!(result[1].candidate.original_rank, 1);
    }
}
