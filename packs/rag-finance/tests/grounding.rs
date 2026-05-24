//! Grounding tests for the finance RAG pack.
//!
//! Each test ingests a seed corpus document into an `InMemoryRagClient`,
//! issues a task-specific retrieval query, and asserts:
//!   (a) at least one hit is returned, and
//!   (b) the top hit's `source_uri` matches the expected seed document.
//!
//! Ratio computation accuracy is validated in the eval harness
//! under `tests/eval/` — these tests verify retrieval, not math.

use std::path::Path;

use xiaoguai_rag::{InMemoryRagClient, IngestRequest, RagClient, SearchRequest};

async fn ingest_corpus_file(
    client: &InMemoryRagClient,
    collection_id: &str,
    relative_path: &str,
) -> String {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("packs/rag-finance");
    let full_path = base.join(relative_path);
    let content = tokio::fs::read_to_string(&full_path)
        .await
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", full_path.display()));
    let source_uri = format!("file://packs/rag-finance/{relative_path}");
    client
        .ingest(IngestRequest {
            collection_id: collection_id.into(),
            source_uri: source_uri.clone(),
            content,
            metadata: serde_json::json!({ "pack": "rag-finance" }),
        })
        .await
        .unwrap_or_else(|e| panic!("ingest failed for {relative_path}: {e}"));
    source_uri
}

/// Verify that a current-ratio query retrieves the balance sheet document,
/// which is the source of both current assets and current liabilities values
/// required to compute the ratio.
#[tokio::test]
async fn ratio_extract_current_ratio_hits_balance_sheet() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-ratio";

    let bs_uri = ingest_corpus_file(&client, coll, "corpus/02-10k-balance-sheet-sample.md").await;
    // Decoy: income statement.
    ingest_corpus_file(&client, coll, "corpus/01-10k-income-statement-sample.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "current assets current liabilities liquidity ratio balance sheet".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for current ratio query"
    );
    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &bs_uri, "expected top hit from balance sheet");
}

/// Verify that a gross-margin query retrieves the income statement document.
#[tokio::test]
async fn ratio_extract_gross_margin_hits_income_statement() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-gross-margin";

    let is_uri =
        ingest_corpus_file(&client, coll, "corpus/01-10k-income-statement-sample.md").await;
    ingest_corpus_file(&client, coll, "corpus/02-10k-balance-sheet-sample.md").await;
    ingest_corpus_file(&client, coll, "corpus/04-financial-ratios-reference.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "gross profit margin revenue cost of revenues income statement".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for gross margin query"
    );
    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &is_uri, "expected top hit from income statement");
}

/// Verify that an MD&A summarization query retrieves the MD&A document.
#[tokio::test]
async fn mdna_summarize_revenue_drivers_hits_mdna() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-mdna";

    let mdna_uri = ingest_corpus_file(&client, coll, "corpus/03-10k-mdna-sample.md").await;
    ingest_corpus_file(&client, coll, "corpus/06-10q-quarterly-report-sample.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "revenue growth drivers management discussion analysis ARR cloud".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for MD&A summary query"
    );
    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &mdna_uri, "expected top hit from MD&A document");
}

/// Verify that a free-cash-flow query retrieves the cash flow statement.
#[tokio::test]
async fn ratio_extract_free_cash_flow_hits_cash_flow_statement() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-fcf";

    let cf_uri =
        ingest_corpus_file(&client, coll, "corpus/07-cash-flow-statement-sample.md").await;
    ingest_corpus_file(&client, coll, "corpus/01-10k-income-statement-sample.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "free cash flow operating activities capital expenditure purchases PP&E".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for free cash flow query"
    );
    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(top_uri, &cf_uri, "expected top hit from cash flow statement");
}

/// Verify that an IFRS/GAAP question retrieves the comparison reference.
#[tokio::test]
async fn mdna_summarize_ifrs_gaap_difference_hits_comparison_doc() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-ifrs";

    let ifrs_uri =
        ingest_corpus_file(&client, coll, "corpus/05-ifrs-gaap-comparison.md").await;
    ingest_corpus_file(&client, coll, "corpus/04-financial-ratios-reference.md").await;

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "IFRS GAAP difference LIFO inventory development capitalization lease".into(),
            top_k: 5,
            min_score: None,
        })
        .await
        .expect("search must not error");

    assert!(
        !result.hits.is_empty(),
        "expected at least one hit for IFRS/GAAP query"
    );
    let top_uri = &result.hits[0].citation.source_uri;
    assert_eq!(
        top_uri, &ifrs_uri,
        "expected top hit from IFRS/GAAP comparison doc"
    );
}

/// Verify citation contract across the full finance corpus.
#[tokio::test]
async fn all_hits_satisfy_citation_contract() {
    let client = InMemoryRagClient::new();
    let coll = "finance-test-citation-contract";

    for file in [
        "corpus/01-10k-income-statement-sample.md",
        "corpus/02-10k-balance-sheet-sample.md",
        "corpus/03-10k-mdna-sample.md",
        "corpus/04-financial-ratios-reference.md",
        "corpus/05-ifrs-gaap-comparison.md",
    ] {
        ingest_corpus_file(&client, coll, file).await;
    }

    let result = client
        .search(SearchRequest {
            collection_id: coll.into(),
            query: "operating margin revenue growth".into(),
            top_k: 10,
            min_score: None,
        })
        .await
        .expect("search must not error");

    for hit in &result.hits {
        let cit = &hit.citation;
        assert!(!cit.source_uri.is_empty(), "source_uri must not be empty");
        assert!(cit.span.0 <= cit.span.1, "span start <= end");
        assert!(
            (0.0..=1.0).contains(&cit.score),
            "score in [0, 1], got {}",
            cit.score
        );
        assert!(!cit.preview.is_empty(), "preview must not be empty");
        assert_eq!(cit.collection_id, coll, "collection_id must match");
    }
}
