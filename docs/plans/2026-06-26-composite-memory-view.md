# CompositeMemoryView — Technical Design (orchestrator memory bridge + bounded multi-source unification)

**Status:** DESIGN — review checkpoint (2026-06-26). No code is written. This doc un-defers the `CompositeMemoryView` that [T7 memory-multisource](2026-06-10-memory-multisource.md §0, §3, §4.3) explicitly shelved, and it addresses the *two prerequisites the deferral named*: (1) "the orchestrator memory bridge does not exist yet" and (2) "an eval can guard context bloat."

**Context.** T7 (2026-06-10) shipped team glossary + memory import/export + `source:` tags, and deferred the read-time unification of xiaoguai's three separate memory stores. The deferral was correct: the production `MemoryView` bridge the orchestrator's triangle pattern declares (`triangle/memory_view.rs:8`) is still a stub, and naively concatenating semantic-memory + IM-history + RAG into every turn would balloon context. This doc designs the bridge, a *bounded* composite reader, and the bloat eval that gates it.

> Methodology (per the repo's "verify before citing in design docs" rule, mirrored from the [Phase 4 doc](2026-06-25-skill-pack-loader-phase4.md)): every code fact below was grep-checked on 2026-06-26 and is labelled **V**. Recommendations are **REC**. §B6 records where the source comment **over-promises** — do not propagate the simpler story.

---

## A. The question this design answers

xiaoguai keeps **three deliberately-separate memory stores with different semantics** (T7 §0):

1. **Semantic memory** — long-term facts/episodes/preferences, embedded + recalled by cosine similarity.
2. **IM history** — the trailing conversational window for an IM conversation (Slack/Feishu/…).
3. **RAG** — document-grounded retrieval over ingested collections, with citations.

Today **none of the three is injected at agent-turn time** — `run_turn` only prepends the owner identity (`USER.md`) and the team glossary (§B6). Recall, IM snapshots, and RAG search each have their own call-sites and their own consumers, and the orchestrator's triangle pattern asks for a `MemoryView` it never receives in production. **The deferred work = a single bounded read that unifies all three at injection time, with source tags and a hard token ceiling, plus the production bridge that feeds it to the orchestrate path.**

This is **read-time only**. We do not merge the stores, do not change their write paths, and do not promote facts between them (T7 §4.3 left promotion to future work — out of scope here too).

---

## B. Verified current surface (grep-checked 2026-06-26)

### B1. The `MemoryView` bridge is a test stub; the production impl is **absent** — **V**
- `crates/xiaoguai-orchestrator/src/triangle/memory_view.rs` defines `trait MemoryView { async fn snapshot(&self, round: u32) -> MemorySnapshot }` (`:32`), value types `MemoryFact{key,value}` (`:19`) + `MemorySnapshot{round,facts,captured_at}` (`:25`), and **only** `InMemoryMemoryView` (`:43`) — its own doc comment calls it an "in-memory test fixture … **Not safe for production use**" (`:39`).
- The module header states the production impl "**will be in `xiaoguai-core::orchestrator_bridge`**" (`:8`). **That file does not exist** — `crates/xiaoguai-core/src/` has `memory_bridge.rs`, `acp_bridge.rs`, `scheduler_bridge.rs`, … but **no `orchestrator_bridge.rs`** (V — directory listing). Every non-test reference to `MemoryView` is inside `crates/xiaoguai-orchestrator/{src/patterns/triangle.rs, tests/triangle_*.rs}` and is fed `InMemoryMemoryView::new()`. **CONFIRMED: no production bridge.**

### B2. The triangle `MemoryView` contract is a per-round snapshot — **V**
- `patterns/triangle.rs` holds `memory: Arc<dyn MemoryView>` (`:221,241,267,326`) and captures **one snapshot per plan→execute round** shared by Planner/Worker/Critic (`memory_view.rs:1-6` invariant: "the same snapshot is read by all three roles for the lifetime of one plan→execute round"). So the bridge's job at orchestrate time is: *materialise a stable `MemorySnapshot` once per round*.
- ⚠️ The current `MemoryFact{key,value}` shape is a **flat KV pair** with no source tag, no score, no recency. The composite reader needs richer snippets (source tag + score). **REC (§C3):** extend `MemoryFact` (or add a sibling `MemorySnippet`) with `source` + `score` rather than overloading `key`.

### B3. Semantic memory store — **V**
- `crates/xiaoguai-memory/src/traits.rs` — `trait MemoryStore` (`:16`) with `recall_memories(RecallRequest) -> Vec<RecalledMemory>` (`:44`). `RecallRequest{query, top_k, kind_filter, tag_filter, session_id}` (`types.rs:111`); `RecalledMemory{memory: Memory, score: f32}` (`:124`) where `score` is "Cosine similarity in [0,1]" and `Memory{content, tags, ttl_at, created_at, last_recalled_at, recall_count, …}` (`:52`). **A recall already returns a bounded, scored, tagged list** — ideal composite input.
- Held on state as `AppState.memory_store: Option<Arc<dyn MemoryStore>>` (`crates/xiaoguai-api/src/state.rs:276`), built in `xiaoguai-core` via `memory_bridge::build_memory_store` (`lib.rs:868`). **`recall_memories` is NOT called from `turn.rs`** (V — grep of turn.rs for `memory_store|recall` is empty); its only callers are the REST routes `routes/memory.rs` + `routes/mod.rs`. So semantic memory is reachable but **not wired into turns today**.

### B4. RAG client — **V**
- `crates/xiaoguai-rag/src/client.rs` — `trait RagClient` (`:29`) with `search(SearchRequest) -> SearchResult` (`:39`). `SearchRequest{collection_id, query, top_k, min_score}` (`types.rs:50`) — **note `collection_id` is required**; `SearchResult{hits: Vec<SearchHit>, elapsed_ms}` (`:76`); `SearchHit{citation: Citation{source_uri, span, score, preview, collection_id}, document_id}` (`:68`, `:30`). Already bounded (`top_k`), scored, and carries provenance (`source_uri`).
- ⚠️ **RAG is NOT on `AppState`** (V — grep of state.rs for `RagClient|rag` is empty). The only production `RagClient` is constructed in `xiaoguai-core` as `InMemoryRagClient::new()` for the **reindex scheduler** (`lib.rs:539-540`, `scheduler_bridge::RagReindexExecutor`). There is **no owner-facing wiring of RAG into chat at all today.** This is the single biggest scope risk (§G1): "unify RAG" presupposes RAG is reachable from a turn, and it is not.

### B5. IM history store — **V**
- `crates/xiaoguai-im-gateway/src/history.rs` — `trait ImHistoryStore` (`:73`) with `snapshot(&ConversationIdent) -> Vec<LlmMessage>` (`:77`), scoped by `ConversationIdent{provider, tenant_external_id, user_external_id, conversation_id}` (`:35`). Default impl `ConversationHistory` (`:91`) is an **in-process ring buffer**, `max_turns` default **20** (`:108`); SQLite-backed variant in `sqlite_history.rs`. Returns ready-made `Vec<LlmMessage>` (not snippets) — already the conversation's own trailing window.
- ⚠️ IM history is **keyed by IM conversation, not by an API session.** An interactive `run_turn` (API session) and an IM turn are different identity spaces. So "inject IM history into a turn" only makes sense **when the turn IS an IM turn** — cross-injecting an unrelated IM conversation into an API chat session would be both wrong and a privacy leak (single-owner, but still cross-context). **REC (§F):** IM history is included **only** for IM-originated turns, addressing their own conversation.

### B6. ⚠️ The turn pipeline injects identity + glossary, in a fixed System-frame order — **V** (and the "unify" comment over-promises)
- `crates/xiaoguai-api/src/turn.rs` builds the message stack as `[identity, glossary, loop_note?, ...history]`: glossary inserted at index 0 (`:233-234`), then identity inserted at 0 (`:250-251`) so identity ends up outermost; both are best-effort (a repo failure logs + proceeds, `:238-239`) and **never persisted into history** (`:229,247`). This is the **exact injection seam** the composite reader plugs into (a new System frame between glossary and history).
- The over-promise: T7 §0 frames the deferral as "the unification would be a read-time `CompositeMemoryView`" as if the only blocker were the bridge. Grep shows a **second, larger** gap: **semantic recall and RAG are not wired into turns at all** (§B3, §B4), and RAG is not even on `AppState`. So the honest scope is not "wire one more source" — it is "stand up the read-time retrieval path that does not exist yet, *bounded from day one*." This doc designs that path; it does **not** assume the sources are already one query away.

### B7. Token estimation + budget-split precedents exist — **V**
- `crates/xiaoguai-llm/src/token_count.rs` — `estimate_tokens(&str)` (`:29`, 4-chars/token, **CJK-aware via `chars().count()`**, conservative round-up) + `estimate_message_tokens(&[Message])` (`:47`, adds a 4-token per-message overhead). Dependency-free, already used by the agent's history-compaction path. **This is the meter for both the budget and the eval ceiling** — no new tokenizer dependency (DEC-033-friendly).
- `crates/xiaoguai-orchestrator/src/triangle/budget.rs` — `TriangleBudget::split(parent) -> RoleBudgets` (`:74`) refuses a split that would floor any share to 0 (`BudgetTooSmall`, `:80`). **Precedent for percentage-based budget partitioning with a floor guard** — the composite per-source caps mirror this shape.

### B8. The eval substrate exists but has no size assertion — **V**
- `crates/xiaoguai-eval/` — `EvalCase`/`EvalSuite`/`EvalRunner`/`EvalReport` (`lib.rs:58,63`), `Assertion` enum (`types.rs:96`) with `FinalMessageContains/Regex`, `ToolInvocationCount`, `AgentEventSequence`, `ToolCallSequence`. **There is no "injected-context size ≤ N tokens" assertion** — the bloat eval needs either a new `Assertion::InjectedContextTokensAtMost{max}` variant or a standalone harness test (§D). The runner drives a mock model via `EvalAgentBuilder`, so it can observe the built message stack.

---

## C. Design — the production bridge + bounded composite reader

> **REC headline.** Build `xiaoguai-core::orchestrator_bridge` (the home the stub names) hosting a `CompositeMemoryView` that fans out to the three sources, applies **per-source caps → ranking → a hard total token ceiling → source-tagged snippets**, and returns a `MemorySnapshot`. Wire it into `run_turn` as a new best-effort System frame *and* hand the same view to the orchestrate path as the `Arc<dyn MemoryView>`. **Bounded from the first line of code — there is no "unbounded then optimize" intermediate state.**

### C1. New crate-internal type: `RetrievedSnippet` (REC)
A single, source-tagged unit the reader ranks and budgets over:

```text
RetrievedSnippet {
    source:      SnippetSource,   // Memory | ImHistory | Rag  (the tag)
    text:        String,          // the content to inject
    score:       f32,             // normalized relevance in [0,1] (see C4)
    recency:     Option<DateTime<Utc>>,  // created_at / message time, for the recency tie-break
    provenance:  Option<String>,  // rag source_uri / memory id — for the rendered "[source: …]" line
}
enum SnippetSource { Memory, ImHistory, Rag }
```

Each source adapts its native result into `RetrievedSnippet`:
- **Memory:** `RecalledMemory{memory, score}` → `{source: Memory, text: memory.content, score, recency: memory.created_at, provenance: memory.id}` (§B3).
- **RAG:** `SearchHit{citation}` → `{source: Rag, text: citation.preview, score: citation.score, provenance: citation.source_uri}` (§B4).
- **IM history:** each `LlmMessage` in the trailing window → `{source: ImHistory, text: role+content, score: recency-derived (C4), recency: msg time}` (§B5). IM messages have no relevance score, so they rank by recency only.

### C2. The bounded read algorithm (REC)
`CompositeMemoryView::retrieve(query, ctx) -> Vec<RetrievedSnippet>`:

1. **Per-source fetch with hard caps** (parallel, `tokio::join!`):
   - Memory: `recall_memories(RecallRequest{query, top_k = MEMORY_TOP_K, kind_filter: None, tag_filter: [], session_id})`.
   - RAG: for each **enabled** collection (§F: opt-in), `search(SearchRequest{collection_id, query, top_k = RAG_TOP_K, min_score: Some(RAG_MIN_SCORE)})`; cap total RAG hits at `RAG_TOP_K` across collections.
   - IM history: `snapshot(ident)` then take the trailing `IM_MAX_TURNS` messages **only for IM-originated turns** (§B5/§F); empty for API/loop turns.
2. **Per-source token sub-budget** (mirrors `TriangleBudget::split`, §B7): each source gets a fixed token share of the total ceiling. Drop snippets (lowest-ranked first) until each source fits its share. *This guarantees no single source can crowd out the others even before global ranking.*
3. **Merge + rank** all surviving snippets by the policy in §C4.
4. **Global token ceiling** (§C5): walk the ranked list accumulating `estimate_tokens(text) + PER_SNIPPET_OVERHEAD`; stop at `TOTAL_CTX_TOKEN_CEILING`. Snippets past the ceiling are dropped (not truncated — truncating a snippet mid-sentence is a correctness hazard, same stance as `validate_content` in `memory/types.rs:82`).
5. **Dedup** (§C6) is applied during the merge (step 3) before ranking consumes the budget.
6. Return the surviving, ranked, tagged snippets.

`MemoryView::snapshot(round)` wraps `retrieve` and maps each `RetrievedSnippet` into the snapshot value type (§C3); it caches the result for the round (the triangle invariant, §B2).

### C3. Snapshot shape — extend `MemoryFact`, don't overload it (REC)
The triangle's `MemoryFact{key,value}` (`memory_view.rs:19`) cannot carry a source tag. **REC:** add `source: SnippetSource` and `score: f32` fields to `MemoryFact` (default `Memory`/`1.0` via serde for back-compat with existing `InMemoryMemoryView` tests), or introduce `MemorySnapshot.snippets: Vec<RetrievedSnippet>` alongside the legacy `facts`. Slice 1 (§E) makes this call; the table-tests pin whichever shape lands. **The rendered injection always carries the tag** — see §C7.

### C4. Ranking / recency policy (REC — concrete)
A single comparable **rank score** per snippet so cross-source ordering is deterministic:

```text
rank = w_source * source_weight(source)
     + w_relevance * score                       // cosine / rag score; IM = 0
     + w_recency * recency_decay(age)            // exp half-life decay, normalized [0,1]
```

- `source_weight`: **Memory 1.0, RAG 0.9, ImHistory 0.7** — semantic facts are the most durable/intentional; RAG is grounded but query-dependent; IM trailing window is context, not knowledge.
- `recency_decay(age) = 0.5 ^ (age_days / RECENCY_HALFLIFE_DAYS)` with `RECENCY_HALFLIFE_DAYS = 30`. IM snippets (no relevance signal) lean almost entirely on this term, so the freshest IM turns win their sub-budget.
- Weights `w_source=0.5, w_relevance=0.4, w_recency=0.1` — **starting constants, declared in one `const` block** (no magic numbers, per coding-style). They are *defaults*, tuned against the eval (§D) — the eval is what justifies any change.

Ties break by `recency` (newer first), then by `source` order, then by `text` (stable, deterministic for tests).

### C5. The budget numbers (REC — concrete, the load-bearing decision)
All in a single `composite_budget.rs` `const` block:

| Constant | Value | Rationale |
|---|---|---|
| `TOTAL_CTX_TOKEN_CEILING` | **1500 tokens** | Hard cap on *all* composite-injected context per turn. ~6 KB of text — meaningful recall without dominating an 8–32 K context. The eval asserts the built stack's composite frame never exceeds this. |
| `MEMORY_SUB_BUDGET` | 700 tokens (≈47 %) | Largest share — durable knowledge. |
| `RAG_SUB_BUDGET` | 600 tokens (≈40 %) | Grounding/citations. |
| `IM_SUB_BUDGET` | 200 tokens (≈13 %) | Trailing IM window is the smallest — history already lives in the message list for API turns; this is only for IM turns. |
| `MEMORY_TOP_K` | 6 | Pre-budget fetch cap. |
| `RAG_TOP_K` | 5 | Across all enabled collections. |
| `RAG_MIN_SCORE` | 0.5 | Floor — a weak RAG hit is noise; drop it rather than spend budget. |
| `IM_MAX_TURNS` | 6 | Trailing turns (≪ the store's 20-msg window). |
| `PER_SNIPPET_OVERHEAD` | 8 tokens | The `[source: …]` wrapper line + separators, charged against the ceiling. |

Sub-budgets sum to 1500 = the ceiling (the split-with-floor guard, §B7, refuses a configuration where any share floors to 0). **These are defaults; the eval (§D) is the gate that proves they hold and the lever that re-tunes them.**

### C6. Dedup (REC)
Cheap, before ranking spends budget:
- **Exact-ish:** normalize whitespace + lowercase, hash the first 200 chars; drop later snippets with a colliding hash (keep the highest-ranked occurrence). Catches the common case of a fact that is also a RAG chunk.
- **Cross-source preference on collision:** keep the **Memory** copy over RAG over IM (an intentional fact beats an incidental chunk), regardless of arrival order.
- No embedding-based near-dup in v1 (would re-embed at read time — latency cost, §G2). Flagged as a follow-up if exact-hash proves too weak.

### C7. Rendered injection format (REC)
The composite frame is **one System message** (best-effort, like glossary/identity), so it cannot break the existing frame order:

```text
Relevant context (retrieved for this turn):
[memory] <snippet text>
[memory] <snippet text>
[knowledge: <source_uri>] <rag snippet text>
[recent] <im snippet text>
```

- Source tag is **always present** on every line — the T7 `source:` convention (§1.2) surfaced into the prompt, so the model (and a debugging owner) can see provenance.
- Inserted in `run_turn` **before the identity insertion** so the final order is `[identity, composite_context, glossary, ...history]` — identity stays outermost, composite context sits with the other read-only enrichment frames, never persisted into history (§B6 precedent). (Exact ordering of composite-vs-glossary is a Slice-3 detail; both are enrichment frames inside identity.)
- If `retrieve` returns empty (no sources enabled, nothing over threshold), **no frame is inserted** (zero-cost for the common API-chat-with-no-RAG case).

### C8. Where it plugs in (REC)
- **Turn pipeline:** `run_turn` (`turn.rs`, around the glossary/identity block `:230-251`) calls the composite reader best-effort and inserts the §C7 frame. New optional field `AppState.composite_memory: Option<Arc<CompositeMemoryView>>` (absent in tests/dev → no-op, mirrors `teams`/`memory_store` optionality).
- **Orchestrate path:** `orchestrate_session` (`routes/orchestrate.rs:152`) and the triangle pattern receive the **same** `CompositeMemoryView` as their `Arc<dyn MemoryView>` — closing the §B1 gap. One reader, two consumers.
- **Construction:** `xiaoguai-core::orchestrator_bridge::build_composite_memory_view(memory_store, rag_client, im_history, config)` in `lib.rs` serve wiring, alongside `build_memory_store` (`:868`). **This is also where RAG finally reaches a turn** — the bridge is what puts a `RagClient` in reach of chat (§B4 gap).

---

## D. The context-bloat eval (the gate the deferral required)

T7 deferred until "an eval can guard context bloat." This is that eval. It measures **the size of injected context** and **asserts a ceiling**, in CI, on every change.

### D1. What it measures (REC)
The **token size of the composite-injected System frame(s)** in the built message stack — *not* the model output. The composite reader is deterministic given fixed source contents and a fixed query, so the eval is deterministic (no live model needed for the size assertion).

### D2. Harness shape (REC — two layers)
1. **Unit/property layer (`xiaoguai-core` tests):** seed an `InMemoryMemoryStore` + an `InMemoryRagClient` + an `ImHistoryStore` with **deliberately oversized** content (e.g. 50 memories × 400 chars, 30 RAG hits, 20 IM turns — far more than the budget), call `retrieve`, and assert:
   - `estimate_message_tokens(rendered_frame) <= TOTAL_CTX_TOKEN_CEILING` (the hard gate);
   - each source's contribution `<= its sub-budget`;
   - every snippet carries a `source` tag (no untagged injection);
   - ranking order matches the §C4 policy on a fixed fixture;
   - dedup removes a planted duplicate.
   This is a **property the design cannot regress** — oversized inputs MUST compress to ≤ ceiling.
2. **Eval-suite layer (`xiaoguai-eval`):** add `Assertion::InjectedContextTokensAtMost{max}` (the missing variant, §B8) and a `composite_context` suite of representative cases (a coding question with RAG enabled, a personal-preference question hitting memory, an IM turn with history, a cold query that retrieves nothing). The `EvalAgentBuilder` exposes the built stack; the assertion sums the composite frame's `estimate_message_tokens` and fails the case if it exceeds `max`. Cases run under the mock model (no network), so the suite is CI-cheap.

### D3. What it samples (REC)
A small fixture corpus checked into the repo under `crates/xiaoguai-eval/fixtures/composite/` (or core test fixtures): a handful of synthetic memories, a tiny RAG collection, and a short IM transcript — **synthetic, owner-neutral** (no real owner data; the test-DB / no-real-secrets rule applies). Each eval case names a query + which sources are enabled + the expected ceiling.

### D4. Pass / fail criteria (REC)
- **Hard fail (blocks merge):** any case where the composite frame exceeds `TOTAL_CTX_TOKEN_CEILING`, or any untagged snippet, or a per-source overflow. These run on the default CI gate (Build-and-test), same tier as the other unit tests.
- **Soft signal (report, non-blocking):** `EvalReport.pass_rate` + a printed median/p95 injected-token size across the suite — so a tuning change (e.g. raising a sub-budget) shows its context-size cost in the report without failing the build, exactly the way the eval substrate reports rates today (`lib.rs:22`).
- The eval is the **lever**: any change to the §C5 budget constants must keep the hard assertions green; the soft report shows whether the change moved median injected size in the intended direction.

---

## E. Phased slices — reviewable PRs (TDD, each gated on Build-and-test)

- **Slice 1 — types + the bounded reader, pure (no serve change, no turn change).** Add `RetrievedSnippet`/`SnippetSource`, the `composite_budget.rs` const block, the `CompositeMemoryView` struct + `retrieve` algorithm (§C2/C4/C5/C6), and the `MemoryView` snapshot extension (§C3). Construct over the three `In*` test impls. **Table-tested + the §D1 property tests (oversized-in → ≤-ceiling-out) land here.** *Lowest risk, unblocks everything; nothing is injected anywhere yet.*
- **Slice 2 — the production bridge.** Create `xiaoguai-core::orchestrator_bridge` (the home the stub names, §B1) with `build_composite_memory_view`; give the orchestrate/triangle path a real `Arc<dyn MemoryView>` (closes §B1). Tests: orchestrate round gets a bounded, tagged snapshot from real stores. **No turn-pipeline change yet** — proves the bridge in isolation.
- **Slice 3 — wire into `run_turn` (behind optional state).** Insert the §C7 composite frame in `run_turn` (best-effort, alongside glossary/identity), gated on `AppState.composite_memory` being present; **RAG reaches chat for the first time** via the bridge. Off by default where the field is `None` (dev/tests). Integration tests: frame present + ordered + ≤ ceiling + tagged; absent when no sources enabled.
- **Slice 4 — the eval gate + the `Assertion` variant.** Add `Assertion::InjectedContextTokensAtMost` (§B8) + the `composite_context` eval suite + fixtures (§D2/D3); wire the suite into the eval run so CI exercises it. This slice is what lets us *un-defer with confidence* — it is the gate T7 named. **Could be authored alongside Slice 1's property tests but ships as its own PR so the suite + variant get a focused review.**
- **Slice 5 (separate doc, deferred) — owner controls + RAG-collection selection UX.** Per-collection enable toggles, a budget-tuning surface, and admin-ui visibility into "what got injected." Out of scope here; flagged because Slice 3 hard-codes "all enabled collections" and the owner will want to choose (§F, §G1).

---

## F. DEC-033 guardrails + when NOT to include a source

- **One SQLite, single owner.** All three stores already live in (or front) the one embedded DB; no new store, no Postgres/Redis/queue, **no per-tenant scoping** — the composite reader takes the owner's whole corpus because there is exactly one owner (the `InMemoryMemoryView` doc's "no per-tenant isolation" caveat, `memory_view.rs:41`, is a *non-issue* under DEC-033, not a blocker). No new daemon — the reader runs inline in the turn/orchestrate path.
- **No new tokenizer dependency** — reuse `estimate_tokens` (§B7), DEC-033-friendly.
- **When NOT to include a source (explicit, REC):**
  - **RAG** — exclude when no collection is enabled for chat, or every hit is below `RAG_MIN_SCORE`. RAG is **opt-in per collection** (§E Slice 5); until an owner enables one, the RAG sub-budget is simply unused. (Today RAG is not on `AppState` at all, §B4 — so "off" is the literal default and Slice 3 introduces the *first* on-switch.)
  - **IM history** — include **only for IM-originated turns**, addressing **that conversation's own** `ConversationIdent` (§B5). Never inject an IM conversation into an unrelated API chat session — different identity space, and a cross-context leak even under single-owner.
  - **Semantic memory** — exclude when the store is empty or recall returns nothing over a relevance floor; never inject zero-score filler just to spend the budget (the empty-frame rule, §C7).
  - **All sources** — the composite frame is **never inserted on a loop tick that opts out**, and never when `retrieve` is empty (zero-cost path).

---

## G. Risks / open questions for review

1. **RAG is not wired to chat at all today (the biggest gap).** §B4: no `RagClient` on `AppState`, only a scheduler-side `InMemoryRagClient`. "Unify RAG" therefore means *introducing* RAG-in-chat, with collection selection and an embedder/backend choice — bigger than "add a source." **REC:** Slice 3 ships with "all enabled collections, none enabled by default"; the real collection-selection UX is Slice 5 (its own doc). Don't let the composite design absorb RAG-backend scope.
2. **Read-time latency.** Three retrievals (one embeds the query for memory recall; RAG may hit a vector backend) on the **hot turn path**. **REC:** parallel fan-out (§C2 step 1), hard `top_k`s, a per-source timeout, and best-effort semantics (a slow/failed source is skipped, the turn proceeds — same posture as the glossary block, `turn.rs:238`). Open question: a global retrieval deadline (e.g. 300 ms) before the turn proceeds without the frame? **REC yes** — flag for review.
3. **Context bloat is the headline risk** and the reason for the deferral. The §C5 ceiling + §D eval are the mitigation; the open question is **the numbers** (1500 total? 47/40/13 split?). These are defaults chosen to be conservative; the eval's soft report (§D4) is how the owner re-tunes with evidence. **Flag for review: is 1500 the right ceiling for an 8 K-context floor?**
4. **`MemoryFact` shape change** (§C3) touches the existing triangle tests. **REC:** additive fields with serde defaults so `InMemoryMemoryView`'s tests stay green; Slice 1 pins the shape. Open question: extend `MemoryFact` vs add a parallel `snippets` field — review preference?
5. **The triangle's once-per-round snapshot vs a turn's once-per-turn read.** The triangle caches a snapshot for a whole plan→execute round (§B2); a chat turn reads once per turn. Same reader, two cache lifetimes. **REC:** the reader is stateless per call; the *triangle* owns its round-cache (as it does today), the turn path just calls `retrieve` once. No shared mutable cache — avoids a staleness foot-gun.
6. **Dedup strength.** Exact-hash (§C6) misses paraphrases. **REC:** ship exact-hash; add embedding near-dup only if the eval shows duplicate snippets eating the budget (don't pay read-time embedding cost speculatively).
7. **Promotion between stores stays out of scope** (T7 §4.3) — this is read-time unification only. IM→memory / RAG→memory promotion is a separate future design.

---

## H. References (grep-verified 2026-06-26)

- `crates/xiaoguai-orchestrator/src/triangle/memory_view.rs:{1,8,19,25,32,39,41,43}` — `MemoryView` trait, `MemoryFact`/`MemorySnapshot`, `InMemoryMemoryView` (test-only, "not safe for production"), production-impl-home comment.
- `crates/xiaoguai-orchestrator/src/patterns/triangle.rs:{221,241,267,326}` — `Arc<dyn MemoryView>` consumers; once-per-round snapshot invariant.
- `crates/xiaoguai-core/src/` — `memory_bridge.rs` present; **no `orchestrator_bridge.rs`** (the named home is absent).
- `crates/xiaoguai-core/src/lib.rs:{539,540,868}` — scheduler-side `InMemoryRagClient`; `build_memory_store` wiring (no composite, no RAG-in-chat).
- `crates/xiaoguai-memory/src/traits.rs:{16,44}`, `types.rs:{52,82,111,124}` — `MemoryStore::recall_memories`, `RecallRequest`/`RecalledMemory`/`Memory`, `validate_content`.
- `crates/xiaoguai-api/src/state.rs:276` — `memory_store: Option<Arc<dyn MemoryStore>>`; **no RAG field**.
- `crates/xiaoguai-rag/src/client.rs:{29,39}`, `types.rs:{30,50,68,76}` — `RagClient::search`, `SearchRequest`(needs `collection_id`)/`SearchHit`/`Citation`.
- `crates/xiaoguai-im-gateway/src/history.rs:{35,73,77,91,108}` — `ImHistoryStore::snapshot`, `ConversationIdent`, `ConversationHistory` (ring buffer, default 20 turns).
- `crates/xiaoguai-api/src/turn.rs:{229,233,238,247,250}` — identity/glossary injection seam, best-effort, never-persisted, frame order.
- `crates/xiaoguai-llm/src/token_count.rs:{29,47}` — `estimate_tokens`/`estimate_message_tokens` (CJK-aware, dependency-free).
- `crates/xiaoguai-orchestrator/src/triangle/budget.rs:{74,80}` — `TriangleBudget::split` floor-guard precedent.
- `crates/xiaoguai-eval/src/{lib.rs:58, types.rs:96}` — `EvalRunner`/`Assertion` (no size assertion yet → §B8/§D2).
- `docs/plans/2026-06-10-memory-multisource.md:{§0,§1.2,§3,§4.3}` — the T7 deferral + its two named prerequisites this doc discharges.
