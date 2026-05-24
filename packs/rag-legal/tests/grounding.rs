//! Grounding tests for the legal RAG pack.
//!
//! Each test ingests a seed corpus document into an `InMemoryRagClient`,
//! issues a representative query, and asserts that:
//!   (a) at least one hit is returned, and
//!   (b) the top hit's `source_uri` matches the expected seed document.
//!
//! The mock-LLM contract: in real deployments the LLM is expected to cite
//! `source_uri` values from `SearchHit::citation`. These tests verify the
//! retrieval half of that pipeline — that the correct document surfaces for
//! each task-specific query. End-to-end citation accuracy (LLM → user) is
//! validated in the eval harness under `tests/eval/`.

use std::path::Path;

use xiaoguai_rag::{InMemoryRagClient, IngestRequest, RagClient, SearchRequest};

/// Helper — ingest a corpus file from the pack directory.
async fn ingest_corpus_file(
    client: &InMemoryRagClient,
    collection_id: &str,
    relative_path: &str,
) -> String {
    // In CI the working directory is the repo root; paths are pack-relative.
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()   // crates/
        .unwrap()
        .parent()   // repo root
        .unwrap()
        .join("packs/rag-legal");
    let full_path = base.join(relative_path);
    let content = tokio::fs::read_to_string(&full_path)
        .await
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", full_path.display()));
    let source_uri = format!("file://packs/rag-legal/{relative_path}");
    client
        .ingest(IngestRequest {
            collection_id: collection_id.into(),
            source_uri: source_uri.clone(),
            content,
            metadata: serde_json::json!({ "pack": "rag-legal" }),
        })
        .await
        .unwrap_or_else(|e| panic!("ingest failed for {relative_path}: {e}"));
    source_uri
}

/// Verify that a clause-extraction query for "warranty disclaimer" retrieves
/// the Apache License or MIT License document, both of which contain AS-IS
/// warranty disclaimers — a canonical legal clause extraction benchmark.
#[tokio::test]
async fn clause_extract_warranty_disclaimer_hits_license_corpus() {
    let client = InMemoryRagClient::new();
    let coll = "legal-test-clause-extract";

    let apache_uri =
        ingest_corpus_file(&client, coll, "corpus/01-apache-license-2.0.md").await;
    let mit_uri = ingest_corpus_file(&client, coll, "corpus/02-mit-license.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "warranty disclaimer AS IS no warranties".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for warranty disclaimer query"
    );

    // Top result must come from one of the two license documents.
    let top_uri = &result.hits[0].citation.source_uri;
    assert!(
        top_uri == &apache_uri || top_uri == &mit_uri,
        "expected top hit from license corpus, got: {top_uri}"
    );
}

/// Verify that a risk-flag query for "uncapped indemnification" retrieves
/// the contract review primer, which contains the indemnification red-flag
/// language used to train the grader.
#[tokio::test]
async fn risk_flag_indemnification_hits_review_primer() {
    let client = InMemoryRagClient::new();
    let coll = "legal-test-risk-flag";

    let primer_uri =
        ingest_corpus_file(&client, coll, "corpus/10-contract-review-primer.md").await;
    // Also ingest NDA so the retriever has competing candidates.
    ingest_corpus_file(&client, coll, "corpus/03-nda-mutual-template.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "uncapped indemnification unlimited exposure red flag".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for indemnification risk query"
    );

    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(
        top_uri, &primer_uri,
        "expected top hit from contract review primer"
    );
}

/// Verify that an NDA-specific query retrieves the NDA template.
#[tokio::test]
async fn clause_extract_nda_confidentiality_hits_nda_template() {
    let client = InMemoryRagClient::new();
    let coll = "legal-test-nda";

    let nda_uri = ingest_corpus_file(&client, coll, "corpus/03-nda-mutual-template.md").await;
    // Decoy: ingest a different agreement.
    ingest_corpus_file(&client, coll, "corpus/04-saas-subscription-agreement.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "confidential information receiving party obligations non-disclosure term".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for NDA confidentiality query"
    );

    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &nda_uri, "expected top hit from NDA template");
}

/// Verify citation contract: every hit must have a non-empty source_uri,
/// a valid span (start ≤ end), and a score in [0, 1].
#[tokio::test]
async fn all_hits_satisfy_citation_contract() {
    let client = InMemoryRagClient::new();
    let coll = "legal-test-citation-contract";

    for file in [
        "corpus/01-apache-license-2.0.md",
        "corpus/02-mit-license.md",
        "corpus/03-nda-mutual-template.md",
        "corpus/04-saas-subscription-agreement.md",
        "corpus/05-employment-offer-letter.md",
    ] {
        ingest_corpus_file(&client, coll, file).await;
    }

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "termination governing law".into(),
            top_k: 10,
            min_score: None,
        })
        .await
        .expect("search must not error");

    for hit in &result.hits {
        let cit = &hit.citation;
        assert!(!cit.source_uri.is_empty(), "source_uri must not be empty");
        assert!(
            cit.span.0 <= cit.span.1,
            "span start must be <= span end, got {:?}",
            cit.span
        );
        assert!(
            (0.0..=1.0).contains(&cit.score),
            "score must be in [0, 1], got {}",
            cit.score
        );
        assert!(!cit.preview.is_empty(), "preview must not be empty");
        assert_eq!(
            cit.collection_id, coll,
            "collection_id must match the queried collection"
        );
    }
}

/// Verify that a DPA-specific query (GDPR, data breach, subprocessor) retrieves
/// the data processing addendum document.
#[tokio::test]
async fn risk_flag_data_breach_notification_hits_dpa() {
    let client = InMemoryRagClient::new();
    let coll = "legal-test-dpa";

    let dpa_uri = ingest_corpus_file(&client, coll, "corpus/07-data-processing-addendum.md").await;
    ingest_corpus_file(&client, coll, "corpus/06-vendor-services-agreement.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "security incident notification 72 hours GDPR subprocessor".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for DPA breach notification query"
    );

    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &dpa_uri, "expected top hit from DPA template");
}
