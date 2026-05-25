# Runbook: RAG Reranker (v1.2)

Cross-encoder reranking for the two-stage RAG retrieval pipeline.

---

## When to use reranking

| Scenario | Recommendation |
|---|---|
| Knowledge-base Q&A, < 200ms latency budget | `NullReranker` (retrieval only) |
| General RAG with moderate latency tolerance (< 1 s) | `CohereReranker` or `VoyageReranker` |
| Multilingual content | `JinaReranker` (jina-reranker-v2-base-multilingual) |
| Air-gapped / no external API allowed | `LlmReranker` (uses the deployed LLM) |
| Development / tests | `NullReranker` |

**Rule of thumb**: add a reranker when precision@5 matters more than throughput.
The two-stage design (large `k_initial` retrieval → small `top_k` rerank output)
typically improves NDCG@5 by 10–20% over single-stage retrieval alone.

---

## Two-stage retrieval architecture

```
User query
    │
    ▼
RagClient.search(k_initial = 50)   ← stage 1: fast ANN retrieval
    │
    ├─ 50 candidates (SearchHit[])
    │
    ▼
Reranker.rerank(top_k = 5)         ← stage 2: cross-encoder scoring
    │
    └─ 5 Scored results → LLM context
```

The plumbing is in `xiaoguai_rag::two_stage_retrieve`.

---

## Configuration knobs

```rust
use xiaoguai_rag::RerankerConfig;

let cfg = RerankerConfig {
    k_initial: 50,       // candidates pulled from the retriever (default: 50)
    top_k: 5,            // results returned after reranking     (default: 5)
    timeout_ms: 5_000,   // per-call API timeout in ms           (default: 5 000)
};
```

All three knobs are also serialisable to/from YAML/JSON via `serde`:

```yaml
# operator.yaml (example)
rag:
  reranker:
    k_initial: 50
    top_k: 5
    timeout_ms: 3000
```

---

## Cost trade-offs

| Provider | Pricing model | Typical cost @ 50 candidates, 1 query |
|---|---|---|
| Cohere `rerank-3.5` | $2 / 1 000 queries | ~$0.002 |
| Voyage `rerank-2` | $0.05 / 1 M tokens (~100 tok/candidate) | ~$0.00025 |
| Jina `jina-reranker-v2-base-multilingual` | Free tier 1M tokens/month, then pay-per-use | ~$0.0002 |
| `LlmReranker` | Billed as LLM tokens (prompt + completion) | Varies; typically 1–3k tokens per query |
| `NullReranker` | $0 | — |

Prices are indicative (2025-06). Check provider pricing pages for current rates.

**Latency** (P50 over a stable connection):

| Provider | P50 latency (50 candidates) |
|---|---|
| `CohereReranker` | ~150 ms |
| `VoyageReranker` | ~120 ms |
| `JinaReranker` | ~200 ms |
| `LlmReranker` | ~800 ms – 2 s (model-dependent) |
| `NullReranker` | < 1 ms |

---

## Latency budget and fallback

Every provider implementation honours `timeout_ms`. On timeout:

1. A `WARN` log is emitted via `tracing`:
   ```
   WARN xiaoguai_rag::reranker: rerank timeout — falling back to retrieval order provider="cohere" timeout_ms=5000
   ```
2. Candidates are returned in the **original retrieval order** with relevance
   scores equal to the citation scores from stage 1.
3. The call site receives a valid `Vec<Scored>` — no error is propagated.

To tune the timeout:

```rust
let cfg = RerankerConfig { timeout_ms: 2_000, ..Default::default() };
```

Network errors and non-2xx responses from the provider API are handled the
same way (log + fallback).

---

## Wiring into the pipeline

### With the convenience function

```rust
use xiaoguai_rag::{
    R2RClient, CohereReranker, RerankerConfig, two_stage_retrieve,
};

let retriever = R2RClient::new("http://localhost:7272");
let reranker = CohereReranker::new(std::env::var("COHERE_API_KEY").unwrap());
let cfg = RerankerConfig::default();

let results = two_stage_retrieve(
    &retriever,
    &reranker,
    "my-collection",
    "user query text",
    &cfg,
).await?;

// results: Vec<Scored>, sorted by descending relevance
for r in &results {
    println!("{:.2}  {}",
        r.relevance,
        r.candidate.hit.citation.source_uri
    );
}
```

### Composing manually

```rust
use xiaoguai_rag::{Candidate, VoyageReranker, Reranker};

let hits = retriever.search(req).await?.hits;
let candidates = Candidate::from_hits(hits);
let reranker = VoyageReranker::new(api_key);
let scored = reranker.rerank(&query, candidates, 5, 5_000).await;
```

### Using LlmReranker

```rust
use std::sync::Arc;
use xiaoguai_rag::LlmReranker;
use xiaoguai_llm::OllamaBackend;

let llm = Arc::new(OllamaBackend::new("http://localhost:11434"));
let reranker = LlmReranker::new(llm, "llama3.2");
```

### Using NullReranker (dev / no-cost)

```rust
use xiaoguai_rag::{NullReranker, two_stage_retrieve, RerankerConfig};

let scored = two_stage_retrieve(&retriever, &NullReranker, "coll", "q", &Default::default()).await?;
```

---

## Observability

All four API providers emit `tracing` events on the warn path:

| Event | Field | Value |
|---|---|---|
| Timeout | `provider`, `timeout_ms` | provider name, configured budget |
| HTTP error | `provider`, `err` | provider name, error string |
| Non-2xx | `provider`, `status` | provider name, HTTP status |
| Parse error | `provider`, `err` | provider name, serde error |

To surface these in your deployment:

```bash
RUST_LOG=xiaoguai_rag::reranker=warn cargo run
```

---

## Deferred: local ONNX cross-encoder (v1.3)

A `LocalCrossEncoderReranker` backed by ONNX Runtime is planned for v1.3.
It will accept a local model path (e.g. a HuggingFace cross-encoder converted
to ONNX) and run fully in-process with zero API cost and ~10–30 ms latency.

Deferred because it adds the `ort` native dependency to the build, which
requires a matching ONNX Runtime shared library in every deployment target.
