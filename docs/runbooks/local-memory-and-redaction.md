# Runbook: air-gapped long-term memory + audit PII redaction

Two operator-facing knobs shipped on branch
`feat/local-memory-and-pii-redaction` (commit `4b56e6d`):

1. **Air-gapped / local long-term memory** — the `/v1/memories` API is now
   served, with the embedding backend selected by the `OLLAMA_HOST` env var.
2. **Audit PII redaction** — emails, IPv4 addresses, `Bearer` tokens, and AWS
   access-key ids are scrubbed from audit entries before they are HMAC-signed.
   On by default; toggled by `XIAOGUAI_AUDIT_REDACT_PII`.

Rust source is not touched here. Adjacent runbooks:
- `docs/runbooks/operator.md` — day-2 ops + the audit chain in depth
- `docs/runbooks/rag-reranker.md` — the *other* air-gapped option (RAG vs memory)

---

## Table of Contents

1. [Long-term memory: backend selection](#1-long-term-memory-backend-selection)
2. [Enabling air-gapped (Ollama) memory](#2-enabling-air-gapped-ollama-memory)
3. [Audit PII redaction](#3-audit-pii-redaction)
4. [Troubleshooting](#4-troubleshooting)

---

## 1. Long-term memory: backend selection

The `/v1/memories` family (CRUD + `recall` + `similar/:id`) was previously
wired to return **503 Service Unavailable** because no `memory_store` was
configured. It is now live: at boot, `xiaoguai-core` builds a SQLite-backed
memory store over the embedded `data.db` and selects an embedding backend
from the `OLLAMA_HOST` environment variable.

| `OLLAMA_HOST` | Embedding backend | Notes |
|---|---|---|
| **set** (non-blank) | `OllamaEmbedder` (air-gapped, local) | Model `all-minilm`, 384-dim. No API key, no outbound cloud call. Requires a reachable Ollama with the model pulled. |
| **unset / blank** | `InMemoryEmbedder` | Deterministic, dependency-free, in-process. **Carries no semantic meaning** — fine for smoke tests and local boot, not for production recall quality. |

Both backends produce **384-dimensional** vectors. Embeddings are stored
as a `content_embedding` BLOB column (384 × `f32`, little-endian) created
by migration `0019_memories.sql`. There is no vector-database extension:
similarity is computed by a brute-force cosine pass in Rust over the
owner's stored vectors, which is sub-millisecond at single-user scale.
Because the dimension is fixed by the encoder either way, **switching
backends needs no schema change** — only a restart.

> **Prerequisite (both backends):** none beyond the embedded SQLite
> `data.db`. Migration `0019_memories.sql` just creates the `memories`
> table with the embedding BLOB column — there is no extension to install
> and no external datastore to provision.

### Companion behaviour — `OLLAMA_HOST` also repoints the chat backend

`OLLAMA_HOST` is read in **two** places at boot, by design:

1. **Memory embedder** (above): set → `OllamaEmbedder`; unset → `InMemoryEmbedder`.
2. **Chat LLM provider**: if set (non-blank), it overrides the `endpoint` of
   the seeded `ollama-local` provider row — without a SQL change.

Migration `0020_ollama_default.sql` promotes Ollama to the **system default
LLM backend**: the server default model becomes **`qwen2.5-coder`**, served by
the `ollama-local` provider at `http://localhost:11434` (cloud providers stay
registered as fallbacks for *their* models — they are not deleted). So a single
`OLLAMA_HOST` value repoints **both** the chat model and the memory embedder at
the same Ollama server.

If you point `OLLAMA_HOST` at a remote GPU box, that box must serve **both**
models:

```bash
ollama pull qwen2.5-coder   # chat / default agent model (migration 0020)
ollama pull all-minilm      # memory embeddings (384-dim)
```

---

## 2. Enabling air-gapped (Ollama) memory

On the host running Ollama (default `http://localhost:11434`):

```bash
# 1. Pull the embedding model used by the memory store (384-dim):
ollama pull all-minilm

# 2. (If you also want the local chat default from migration 0020)
ollama pull qwen2.5-coder
```

Then point xiaoguai-core at it:

```bash
# Local Ollama on the same host:
export OLLAMA_HOST=http://localhost:11434

# ...or a dedicated GPU box:
export OLLAMA_HOST=http://gpu-box.internal:11434
```

`OLLAMA_HOST` is trimmed; surrounding whitespace is ignored. A blank value
(e.g. `OLLAMA_HOST=""`) falls back to the in-process `InMemoryEmbedder`, the
same as leaving it unset.

Restart so the embedder is rebuilt:

```bash
# systemd:
sudo systemctl restart xiaoguai-core
# docker-compose:
docker compose -f deploy/docker-compose.yml restart xiaoguai-core
```

Confirm the selection in the logs — `build_memory_store` logs the chosen
backend at boot:

```bash
docker compose -f deploy/docker-compose.yml logs xiaoguai-core \
  | grep "memory: selected embedding backend"
# → choice=Ollama("http://localhost:11434")   when OLLAMA_HOST is set
# → choice=InMemory                            when it is unset/blank
```

Smoke-test that `/v1/memories` is live (no longer 503):

```bash
# Pass owner credentials with HTTP Basic if auth is configured;
# omit -u when the instance is open on localhost.
curl -s "http://localhost:7600/v1/memories" \
  -u "$XIAOGUAI_AUTH__USERNAME:$XIAOGUAI_AUTH__PASSWORD" | jq
# → 200 with a (possibly empty) list, NOT 503
```

### Using a non-default embedding model (dimension change)

`all-minilm` is the only model the stock build embeds with. The
`content_embedding` BLOB column itself is dimension-agnostic — it stores
whatever `f32` vector you write — but the brute-force cosine pass assumes
all stored vectors share one dimension. Mixing models with different
dimensions in the same `data.db` produces meaningless similarity scores
(and the cosine pass treats a length mismatch as no match), so a model
swap is effectively a re-embed of the whole table, not an in-place change:

| Model | Dimensions | Notes |
|---|:---:|---|
| `all-minilm` | 384 | default |
| `nomic-embed-text` | 768 | re-embed all rows after switching |
| `mxbai-embed-large` | 1024 | re-embed all rows after switching |

The boot-time backend selection always builds `OllamaEmbedder` with the
`all-minilm`/384-dim defaults; using another model is not an env toggle today
(it needs a code change). If you switch, clear and re-embed existing
`memories` rows so the BLOBs are all the same dimension.

---

## 3. Audit PII redaction

Before each audit entry is HMAC-signed and persisted, a `Redactor` scrubs
PII/secret substrings out of it. Because the persisted row **and** its
signature are both computed over the redacted form, the HMAC chain
(`/v1/admin/audit/verify`) stays verifiable — redaction does not break the
chain.

### What gets scrubbed

| Pattern | Replaced with | Note |
|---|---|---|
| Email addresses | `[redacted-email]` | |
| IPv4 addresses | `[redacted-ip]` | |
| `Bearer <token>` (case-insensitive) | `Bearer [redacted-token]` | the `Bearer` scheme word is kept; only the token is dropped |
| AWS access-key ids (`AKIA…`) | `[redacted-token]` | |

Redaction applies to the `actor`, `resource`, and the string **values** nested
inside `details` (recursively, including arrays). JSON object **keys** are
preserved, so the structure of `details` stays intact — only leaf string values
are scrubbed.

### What is NOT touched

One field passes through verbatim, by design:

- **`action`** — a fixed verb (`session.create`, `tool.invoke`, …), never PII.

Redaction is immutable: it returns a new entry; the input is never mutated.

### Toggle — `XIAOGUAI_AUDIT_REDACT_PII`

**On by default** (the enterprise privacy posture). To disable, set the env
var to one of `false` / `0` / `no` / `off` (case-insensitive, trimmed):

```bash
# Disable redaction (e.g. you need raw IPs in audit for forensics):
export XIAOGUAI_AUDIT_REDACT_PII=false
```

Any other value — including unset, empty, or `true`/`1`/`on` — leaves
redaction **enabled**. The choice is logged at boot:

```bash
docker compose -f deploy/docker-compose.yml logs xiaoguai-core \
  | grep -i "audit PII redaction"
# → serve: audit PII redaction ENABLED (XIAOGUAI_AUDIT_REDACT_PII)
# → serve: audit PII redaction DISABLED via XIAOGUAI_AUDIT_REDACT_PII
```

> **Caveat:** the toggle takes effect at the boundary where new entries are
> signed. Rows written under one setting are not re-signed when you flip it;
> the chain remains valid because each row's signature already matches its own
> (redacted-or-not) on-disk form.

---

## 4. Troubleshooting

**1. `/v1/memories` returns 503.** The `memory_store` was not wired. This
should no longer happen on this branch (it is always built at boot over the
embedded `data.db`). If you still see 503, the build is from before the
bridge landed, or the database failed to open — check the boot logs for the
`memory: selected embedding backend` line; if it is absent, the bridge never
ran.

**2. Ollama embedding calls fail.** Recall / create return an embedding error
mentioning the Ollama URL or HTTP status. The error text already names the fix:

```
Ollama HTTP request failed (is Ollama running at http://...?): ...
Ollama returned HTTP 404 — check that the model is available (`ollama pull all-minilm`).
```

Verify the model is pulled on the host that `OLLAMA_HOST` points at:

```bash
curl -s http://$OLLAMA_HOST_NO_SCHEME/api/tags | jq '.models[].name'
# expect: "all-minilm:latest" (and "qwen2.5-coder:latest" for chat)
```

**3. Dimension mismatch on recall.** The BLOB column accepts any length, but
the brute-force cosine pass skips (or scores meaninglessly) vectors whose
length ≠ the query's. Cause: an embedding model other than `all-minilm` was
used, so the `data.db` now holds mixed-dimension BLOBs. Either revert to
`all-minilm` or clear and re-embed every `memories` row at the new dimension
(see §2).

**4. Recall quality is poor / nonsensical with `OLLAMA_HOST` unset.** That is
the `InMemoryEmbedder` — a deterministic hash, not a semantic model. It exists
so the server boots and `/v1/memories` is live without an external dependency;
it is **not** for production recall. Set `OLLAMA_HOST` (and pull `all-minilm`)
for real semantic memory.

**5. PII still appears in `audit_log`.** Confirm redaction is actually on
(boot log line above). Note the documented exception: `action` is
never redacted, and the pattern set is conservative (emails, IPv4,
`Bearer` tokens, AWS keys) — IPv6, other token schemes, and free-form secrets
are out of scope for this release. Do **not** `DELETE`/`UPDATE` rows to scrub
them after the fact: that breaks the append-only chain (see
`docs/runbooks/operator.md` → Audit chain).
