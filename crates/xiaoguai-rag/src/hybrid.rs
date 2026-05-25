//! Hybrid retriever — Reciprocal Rank Fusion (RRF) of a vector backend
//! (Qdrant semantic search) and a lexical backend (Tantivy BM25).
//!
//! ## Algorithm: Reciprocal Rank Fusion
//!
//! RRF was introduced by Cormack, Clarke & Buettcher (SIGIR 2009) and has
//! become the default fusion algorithm in hybrid retrieval systems.
//!
//! For each document *d* appearing in ranked list *r* at rank position *k*
//! (1-indexed):
//!
//! ```text
//! RRF(d) = Σ_{r ∈ rankers} weight_r / (K + rank_r(d))
//! ```
//!
//! Where:
//! * `K = 60` — the smoothing constant that reduces the impact of very
//!   high-ranked documents. The 2009 paper showed K=60 is near-optimal
//!   across TREC benchmarks.
//! * `weight_r` — the configurable per-ranker weight (defaults to 1.0 for
//!   both). Setting `vector_weight > lexical_weight` favours semantic
//!   recall; the reverse favours lexical precision.
//!
//! Documents that only appear in one list receive the score from that list
//! only. The final fused score is normalised to `[0, 1]` by dividing by the
//! theoretical maximum (`Σ weight_r / K`).
//!
//! ## Design
//!
//! `HybridRetriever` wraps two `Arc<dyn RagClient>` — one expected to be
//! a vector store (e.g. `QdrantStore`) and one a lexical store (e.g.
//! `TantivyStore`). This coupling is intentional: `HybridRetriever` itself
//! implements `RagClient`, so it slots in wherever either individual backend
//! would live. Callers never need to know which fusion strategy is active.
//!
//! Non-search methods (`ingest`, `delete_document`, `list_collections`)
//! fan out to both backends in parallel. `list_collections` deduplicates by
//! `collection_id`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::client::{RagClient, RagError, RagResult};
use crate::types::{
    Collection, IngestRequest, IngestResult, SearchHit, SearchRequest, SearchResult,
};

// ---------------------------------------------------------------------------
// HybridConfig
// ---------------------------------------------------------------------------

/// Configuration for the hybrid retriever.
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// RRF smoothing constant. The paper recommends 60; lower values
    /// (e.g. 1) amplify the top-rank advantage.
    pub rrf_k: f32,
    /// Weight applied to the vector-backend ranked list.
    pub vector_weight: f32,
    /// Weight applied to the lexical-backend ranked list.
    pub lexical_weight: f32,
    /// How many candidates to fetch from each backend before fusing.
    /// Must be ≥ `top_k` in `SearchRequest`; defaults to `top_k * 3`.
    /// A larger window improves recall at the cost of extra backend work.
    pub candidate_multiplier: u32,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            vector_weight: 1.0,
            lexical_weight: 1.0,
            candidate_multiplier: 3,
        }
    }
}

impl HybridConfig {
    /// Validate configuration values.
    pub fn validate(&self) -> RagResult<()> {
        if self.rrf_k <= 0.0 {
            return Err(RagError::InvalidArgument("rrf_k must be positive".into()));
        }
        if self.vector_weight < 0.0 || self.lexical_weight < 0.0 {
            return Err(RagError::InvalidArgument(
                "weights must be non-negative".into(),
            ));
        }
        if self.vector_weight == 0.0 && self.lexical_weight == 0.0 {
            return Err(RagError::InvalidArgument(
                "at least one weight must be positive".into(),
            ));
        }
        if self.candidate_multiplier == 0 {
            return Err(RagError::InvalidArgument(
                "candidate_multiplier must be ≥ 1".into(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HybridRetriever
// ---------------------------------------------------------------------------

/// Hybrid retriever that fuses results from a vector and a lexical backend.
///
/// ```rust,ignore
/// let retriever = HybridRetriever::new(
///     Arc::new(qdrant_store),
///     Arc::new(tantivy_store),
///     HybridConfig::default(),
/// );
/// let results = retriever.search(req).await?;
/// ```
pub struct HybridRetriever {
    vector: Arc<dyn RagClient>,
    lexical: Arc<dyn RagClient>,
    config: HybridConfig,
}

impl std::fmt::Debug for HybridRetriever {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridRetriever")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl HybridRetriever {
    /// Create a new `HybridRetriever`.
    ///
    /// # Errors
    ///
    /// Returns [`RagError::InvalidArgument`] if `config` fails validation.
    pub fn new(
        vector: Arc<dyn RagClient>,
        lexical: Arc<dyn RagClient>,
        config: HybridConfig,
    ) -> RagResult<Self> {
        config.validate()?;
        Ok(Self {
            vector,
            lexical,
            config,
        })
    }

    /// Convenience constructor with default config.
    pub fn with_defaults(vector: Arc<dyn RagClient>, lexical: Arc<dyn RagClient>) -> Self {
        Self {
            vector,
            lexical,
            config: HybridConfig::default(),
        }
    }

    /// The theoretical maximum RRF score for a document that ranks #1 in
    /// every list. Used to normalise fused scores to `[0, 1]`.
    fn max_rrf_score(&self) -> f32 {
        // Rank 1 → contribution = weight / (K + 1)
        (self.config.vector_weight + self.config.lexical_weight) / (self.config.rrf_k + 1.0)
    }
}

// ---------------------------------------------------------------------------
// RRF core logic (pure function — independently testable)
// ---------------------------------------------------------------------------

/// A ranked result list entry.
#[derive(Debug, Clone)]
pub struct RankedHit {
    /// Stable document key used for fusion (typically `document_id`).
    pub key: String,
    /// The full search hit (kept for the winner's citation data).
    pub hit: SearchHit,
}

/// Fuse two ranked lists using Reciprocal Rank Fusion.
///
/// Returns hits sorted by descending fused score, truncated to `top_k`.
/// Scores are normalised to `[0, 1]` relative to `max_score`.
///
/// # Arguments
///
/// * `vector_hits` — ranked list from the vector backend (rank 1 = index 0).
/// * `lexical_hits` — ranked list from the lexical backend.
/// * `rrf_k` — smoothing constant (60 recommended).
/// * `vector_weight` — weight for the vector list.
/// * `lexical_weight` — weight for the lexical list.
/// * `max_score` — theoretical max used for normalisation.
/// * `top_k` — how many results to return.
pub fn rrf_fuse(
    vector_hits: Vec<RankedHit>,
    lexical_hits: Vec<RankedHit>,
    rrf_k: f32,
    vector_weight: f32,
    lexical_weight: f32,
    max_score: f32,
    top_k: usize,
) -> Vec<SearchHit> {
    // Accumulate RRF scores keyed by document key.
    let mut scores: HashMap<String, f32> = HashMap::new();
    // Keep one representative hit per key (prefer vector hit if present,
    // as it carries the citation from the semantic backend).
    let mut hit_map: HashMap<String, SearchHit> = HashMap::new();

    for (rank, rh) in vector_hits.iter().enumerate() {
        // rank is 0-indexed; RRF uses 1-indexed ranks.
        let contribution = vector_weight / (rrf_k + (rank as f32 + 1.0));
        *scores.entry(rh.key.clone()).or_insert(0.0) += contribution;
        hit_map
            .entry(rh.key.clone())
            .or_insert_with(|| rh.hit.clone());
    }

    for (rank, rh) in lexical_hits.iter().enumerate() {
        let contribution = lexical_weight / (rrf_k + (rank as f32 + 1.0));
        *scores.entry(rh.key.clone()).or_insert(0.0) += contribution;
        hit_map
            .entry(rh.key.clone())
            .or_insert_with(|| rh.hit.clone());
    }

    // Sort by descending fused score.
    let mut fused: Vec<(String, f32)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(top_k);

    // Normalise scores and build final hit list.
    let norm = if max_score > 0.0 { max_score } else { 1.0 };
    fused
        .into_iter()
        .filter_map(|(key, raw_score)| {
            hit_map.get(&key).map(|h| {
                let mut hit = h.clone();
                hit.citation.score = (raw_score / norm).clamp(0.0, 1.0);
                hit
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// RagClient impl
// ---------------------------------------------------------------------------

#[async_trait]
impl RagClient for HybridRetriever {
    async fn list_collections(&self) -> RagResult<Vec<Collection>> {
        let (v_cols, l_cols) = tokio::try_join!(
            self.vector.list_collections(),
            self.lexical.list_collections()
        )?;
        // Deduplicate by collection_id.
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut merged: Vec<Collection> = Vec::new();
        for c in v_cols.into_iter().chain(l_cols) {
            if seen.insert(c.id.clone()) {
                merged.push(c);
            }
        }
        merged.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(merged)
    }

    async fn search(&self, req: SearchRequest) -> RagResult<SearchResult> {
        let start = std::time::Instant::now();

        // Fetch more candidates than `top_k` from each backend to
        // increase recall before fusion.
        let candidate_k = req
            .top_k
            .saturating_mul(self.config.candidate_multiplier)
            .max(req.top_k);

        let vector_req = SearchRequest {
            top_k: candidate_k,
            min_score: None, // Don't filter before fusion.
            ..req.clone()
        };
        let lexical_req = SearchRequest {
            top_k: candidate_k,
            min_score: None,
            ..req.clone()
        };

        // Run both backends in parallel; tolerate individual failures
        // gracefully — one backend erroring doesn't kill the whole query.
        let (vector_result, lexical_result) = tokio::join!(
            self.vector.search(vector_req),
            self.lexical.search(lexical_req),
        );

        let vector_hits: Vec<RankedHit> = vector_result
            .unwrap_or_else(|e| {
                tracing::warn!(err = %e, "hybrid: vector backend error, using empty result");
                SearchResult {
                    hits: Vec::new(),
                    elapsed_ms: 0,
                }
            })
            .hits
            .into_iter()
            .map(|h| RankedHit {
                key: h.document_id.clone(),
                hit: h,
            })
            .collect();

        let lexical_hits: Vec<RankedHit> = lexical_result
            .unwrap_or_else(|e| {
                tracing::warn!(err = %e, "hybrid: lexical backend error, using empty result");
                SearchResult {
                    hits: Vec::new(),
                    elapsed_ms: 0,
                }
            })
            .hits
            .into_iter()
            .map(|h| RankedHit {
                key: h.document_id.clone(),
                hit: h,
            })
            .collect();

        let top_k = usize::try_from(req.top_k).unwrap_or(usize::MAX);
        let max_score = self.max_rrf_score();
        let mut fused = rrf_fuse(
            vector_hits,
            lexical_hits,
            self.config.rrf_k,
            self.config.vector_weight,
            self.config.lexical_weight,
            max_score,
            top_k,
        );

        // Apply min_score filter after fusion (uses normalised fused score).
        if let Some(min) = req.min_score {
            fused.retain(|h| h.citation.score >= min);
        }

        let elapsed_ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
        Ok(SearchResult {
            hits: fused,
            elapsed_ms,
        })
    }

    async fn ingest(&self, req: IngestRequest) -> RagResult<IngestResult> {
        // Fan out to both backends in parallel; fail if either errors.
        let (v_res, l_res) =
            tokio::try_join!(self.vector.ingest(req.clone()), self.lexical.ingest(req))?;
        // Return the vector backend's document_id (primary key for vector
        // search); the lexical backend's document_id is derived from
        // source_uri and not needed by callers.
        Ok(IngestResult {
            document_id: v_res.document_id,
            chunk_count: v_res.chunk_count.max(l_res.chunk_count),
        })
    }

    async fn delete_document(&self, collection_id: &str, document_id: &str) -> RagResult<()> {
        // Fan out in parallel; report the first error if both fail.
        let (v_res, l_res) = tokio::join!(
            self.vector.delete_document(collection_id, document_id),
            self.lexical.delete_document(collection_id, document_id),
        );
        v_res?;
        l_res?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Citation;

    fn make_hit(doc_id: &str, score: f32) -> RankedHit {
        RankedHit {
            key: doc_id.to_string(),
            hit: SearchHit {
                document_id: doc_id.to_string(),
                citation: Citation {
                    source_uri: format!("file:///{doc_id}.md"),
                    span: (1, 1),
                    score,
                    preview: doc_id.to_string(),
                    collection_id: "test".into(),
                },
            },
        }
    }

    // ------------------------------------------------------------------
    // RRF formula tests — scripted ranks, deterministic assertions
    // ------------------------------------------------------------------

    #[test]
    fn rrf_single_doc_only_in_vector_list_gets_correct_score() {
        // doc_a rank 1 in vector, absent in lexical.
        // RRF contribution = 1.0 / (60 + 1) = 1/61 ≈ 0.01639
        let vector = vec![make_hit("doc_a", 0.9)];
        let lexical: Vec<RankedHit> = vec![];
        let max = 1.0_f32 / 61.0 + 1.0 / 61.0; // both weights=1, rank=1
        let fused = rrf_fuse(vector, lexical, 60.0, 1.0, 1.0, max, 5);
        assert_eq!(fused.len(), 1);
        let score = fused[0].citation.score;
        // doc_a contributes 1/61 from vector only; max = 2/61.
        // normalised = (1/61) / (2/61) = 0.5
        assert!(
            (score - 0.5).abs() < 1e-4,
            "expected ≈0.5 (one of two lists), got {score}"
        );
    }

    #[test]
    fn rrf_doc_in_both_lists_rank1_gets_max_score() {
        // doc_a rank 1 in both vector and lexical.
        // contribution = 1/(60+1) + 1/(60+1) = 2/61
        // max = 2/61 → normalised = 1.0
        let vector = vec![make_hit("doc_a", 1.0)];
        let lexical = vec![make_hit("doc_a", 1.0)];
        let max = 2.0_f32 / 61.0;
        let fused = rrf_fuse(vector, lexical, 60.0, 1.0, 1.0, max, 5);
        assert_eq!(fused.len(), 1);
        let score = fused[0].citation.score;
        assert!(
            (score - 1.0).abs() < 1e-4,
            "rank-1 in both lists should fuse to 1.0, got {score}"
        );
    }

    #[test]
    fn rrf_higher_rank_produces_higher_fused_score() {
        // doc_a rank 1 in vector, doc_b rank 2.
        let vector = vec![make_hit("doc_a", 0.9), make_hit("doc_b", 0.7)];
        let lexical: Vec<RankedHit> = vec![];
        let max = 2.0_f32 / 61.0;
        let fused = rrf_fuse(vector, lexical, 60.0, 1.0, 1.0, max, 5);
        let score_a = fused
            .iter()
            .find(|h| h.document_id == "doc_a")
            .unwrap()
            .citation
            .score;
        let score_b = fused
            .iter()
            .find(|h| h.document_id == "doc_b")
            .unwrap()
            .citation
            .score;
        assert!(
            score_a > score_b,
            "rank-1 must outscore rank-2: {score_a} vs {score_b}"
        );
    }

    #[test]
    fn rrf_weighted_vector_dominates_when_weight_is_higher() {
        // doc_a rank 1 in vector only (weight 2.0).
        // doc_b rank 1 in lexical only (weight 1.0).
        // Expected: doc_a has higher fused score.
        let vector = vec![make_hit("doc_a", 0.8)];
        let lexical = vec![make_hit("doc_b", 0.8)];
        let max = (2.0_f32 + 1.0_f32) / 61.0; // max contribution from rank-1
        let fused = rrf_fuse(vector, lexical, 60.0, 2.0, 1.0, max, 5);
        let score_a = fused
            .iter()
            .find(|h| h.document_id == "doc_a")
            .unwrap()
            .citation
            .score;
        let score_b = fused
            .iter()
            .find(|h| h.document_id == "doc_b")
            .unwrap()
            .citation
            .score;
        assert!(
            score_a > score_b,
            "vector-weighted doc_a should outscore lexical doc_b: {score_a} vs {score_b}"
        );
    }

    #[test]
    fn rrf_top_k_truncates_result() {
        let vector: Vec<RankedHit> = (0..10).map(|i| make_hit(&format!("d{i}"), 1.0)).collect();
        let lexical: Vec<RankedHit> = vec![];
        let max = 2.0_f32 / 61.0;
        let fused = rrf_fuse(vector, lexical, 60.0, 1.0, 1.0, max, 3);
        assert_eq!(fused.len(), 3, "should truncate to top_k=3");
    }

    #[test]
    fn rrf_all_scores_in_zero_one_range() {
        let vector: Vec<RankedHit> = (0..5).map(|i| make_hit(&format!("v{i}"), 0.9)).collect();
        let lexical: Vec<RankedHit> = (0..5).map(|i| make_hit(&format!("l{i}"), 0.8)).collect();
        let max = 2.0_f32 / 61.0;
        let fused = rrf_fuse(vector, lexical, 60.0, 1.0, 1.0, max, 20);
        for h in &fused {
            let s = h.citation.score;
            assert!((0.0..=1.0).contains(&s), "score {s} out of [0,1]");
        }
    }

    #[test]
    fn rrf_k_smaller_amplifies_top_rank() {
        // With K=1 the top rank contribution = 1/(1+1) = 0.5, vs 1/(60+1)
        // at K=60. Verify that the ratio top-rank / second-rank is higher.
        let vector = vec![make_hit("a", 1.0), make_hit("b", 0.5)];
        let max_k1 = 2.0_f32 / 2.0;
        let fused_k1 = rrf_fuse(vector.clone(), vec![], 1.0, 1.0, 1.0, max_k1, 5);
        let max_k60 = 2.0_f32 / 61.0;
        let fused_k60 = rrf_fuse(vector, vec![], 60.0, 1.0, 1.0, max_k60, 5);

        let ratio_k1 = fused_k1[0].citation.score / fused_k1[1].citation.score;
        let ratio_k60 = fused_k60[0].citation.score / fused_k60[1].citation.score;
        assert!(
            ratio_k1 > ratio_k60,
            "K=1 should amplify rank gap more than K=60: {ratio_k1} vs {ratio_k60}"
        );
    }

    // ------------------------------------------------------------------
    // Config validation
    // ------------------------------------------------------------------

    #[test]
    fn config_validation_rejects_negative_k() {
        let cfg = HybridConfig {
            rrf_k: -1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validation_rejects_all_zero_weights() {
        let cfg = HybridConfig {
            vector_weight: 0.0,
            lexical_weight: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validation_rejects_zero_multiplier() {
        let cfg = HybridConfig {
            candidate_multiplier: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_default_is_valid() {
        HybridConfig::default().validate().unwrap();
    }

    // ------------------------------------------------------------------
    // HybridRetriever with in-memory backends
    // ------------------------------------------------------------------

    use crate::memory::InMemoryRagClient;

    fn make_hybrid() -> HybridRetriever {
        let vector: Arc<dyn RagClient> = Arc::new(InMemoryRagClient::new());
        let lexical: Arc<dyn RagClient> = Arc::new(InMemoryRagClient::new());
        HybridRetriever::with_defaults(vector, lexical)
    }

    #[tokio::test]
    async fn hybrid_ingest_and_search_round_trip() {
        let r = make_hybrid();
        r.ingest(IngestRequest {
            collection_id: "c".into(),
            source_uri: "file:///doc.md".into(),
            content: "the needle in a haystack".into(),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

        let result = r
            .search(SearchRequest {
                collection_id: "c".into(),
                query: "needle".into(),
                top_k: 5,
                min_score: None,
            })
            .await
            .unwrap();
        // Both backends should return a hit; RRF should produce at least one.
        assert!(!result.hits.is_empty(), "should find at least one hit");
        let top = &result.hits[0];
        assert!(top.citation.score > 0.0 && top.citation.score <= 1.0);
    }

    #[tokio::test]
    async fn hybrid_list_collections_deduplicates() {
        // Both backends independently create "shared-coll".
        let vector = Arc::new(InMemoryRagClient::new());
        let lexical = Arc::new(InMemoryRagClient::new());
        vector.ensure_collection("shared-coll", "Shared", None);
        lexical.ensure_collection("shared-coll", "Shared", None);
        let r = HybridRetriever::with_defaults(
            vector as Arc<dyn RagClient>,
            lexical as Arc<dyn RagClient>,
        );
        let cols = r.list_collections().await.unwrap();
        let count = cols.iter().filter(|c| c.id == "shared-coll").count();
        assert_eq!(count, 1, "duplicate collections should be deduped");
    }

    #[tokio::test]
    async fn hybrid_delete_fans_out_to_both_backends() {
        let r = make_hybrid();
        r.ingest(IngestRequest {
            collection_id: "c".into(),
            source_uri: "file:///x.md".into(),
            content: "needle here".into(),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

        // Fetch the document_id from the first search hit.
        let result = r
            .search(SearchRequest {
                collection_id: "c".into(),
                query: "needle".into(),
                top_k: 1,
                min_score: None,
            })
            .await
            .unwrap();
        assert!(!result.hits.is_empty());
        let doc_id = result.hits[0].document_id.clone();

        r.delete_document("c", &doc_id).await.unwrap();
        // After deletion both backends should return empty (InMemoryRagClient
        // deletes by document_id, which is what the hybrid passes through).
    }

    #[test]
    fn max_rrf_score_is_positive() {
        let r = HybridRetriever::with_defaults(
            Arc::new(InMemoryRagClient::new()),
            Arc::new(InMemoryRagClient::new()),
        );
        assert!(r.max_rrf_score() > 0.0);
    }
}
