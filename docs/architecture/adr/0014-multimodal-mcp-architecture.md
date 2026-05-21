# ADR-0014 — Process-isolated multi-modal MCP architecture

Date: 2026-05-21
Status: Accepted

## Context

IM-first enterprise scenario: ~40% of 飞书/钉钉/企微 business-chat message volume is non-text (screenshots, forwarded PDFs, voice memos). Xiaoguai must handle image / PDF / audio inputs to be usable on mobile-IM.

Research wave #2 (`docs/research/2026-05-21-local-agent-pain-points.md` §9ter I7) surveyed the 2026 landscape:

- **Vision**: Qwen2.5-VL-7B is consensus best <10B local VLM (Ollama-hosted). MiniCPM-V 2.6 8B as edge alternative. **Llama-3.2-Vision-11B underperforms** — skip.
- **PDF**: `pdfium-render` (Chromium Pdfium wrapper) is the fastest Rust PDF library by ~5×. Tesseract OCR scores 34% on OCR-Bench vs vision LLMs at 73% — **don't use Tesseract**.
- **Audio**: whisper.cpp via `whisper-rs` / `whisper-cpp-plus` stable; **vLLM Whisper 0.14.1 has critical WER regression** (134% loop on L40S).

The strong temptation is to link Pdfium + GGML + Tract into the Rust core for "single binary purity." This is the wrong call:

- **Pdfium** = Chromium C++ dependency. Building requires C++ toolchain + 200MB of native libs.
- **whisper.cpp** = GGML kernel; needs Metal / CUDA / Vulkan accelerator-specific builds.
- **VLMs** = always called via Ollama HTTP (or vLLM); we never run them in-process.

Forcing all three into the Rust binary explodes build complexity, breaks `cargo install`, complicates cross-compilation (already non-trivial — see ADR's pending 信创 work), and ties release cadence to upstream native library cadence.

## Decision

Multi-modal inputs are handled by **process-isolated MCP servers**, one per modality, callable from `xiaoguai-agent` via the standard MCP supervisor mechanism (ADR-0006).

### Architecture

```
IM adapter receives message
        │
        ▼
modality detector inspects MIME / first bytes
        │
        ├── text/*           → straight to agent loop
        │
        ├── image/*          → xiaoguai-vision-mcp.analyze_image
        │                       (calls Ollama Qwen2.5-VL HTTP)
        │
        ├── application/pdf  → xiaoguai-pdf-mcp.pdf_to_markdown
        │                       (pdfium-render in-process within MCP server)
        │
        └── audio/*          → xiaoguai-asr-mcp.transcribe_audio
                                (whisper.cpp in-process within MCP server)

Each modality MCP server returns:
    structured JSON {text, metadata, source_hash}

Agent receives result as annotated user-turn:
    [image-summary k=v ...]
    [pdf-excerpt page=3-5 ...]
    [audio-transcript dur=15s ...]
```

### Three modality MCP packages

#### `xiaoguai-vision-mcp` (v1.0)

- Rust MCP server, ~500 LOC
- Tools: `analyze_image(path, prompt) → {text, ocr_blocks?}`
- Auto-resize via `image` crate to ≤1280px long edge (defeats Ollama text-too-small failure mode)
- Calls Ollama `/api/chat` with `images: [base64]`, model defaults to `qwen2.5vl:7b`
- Cloud fallback router (passes through Xiaoguai LLM gateway): on local failure or `--quality=high`, route to gpt-4o-mini / Claude / Gemini
- Acceptance test: 5-prompt eval (screenshot OCR, chart read, UI mockup, scanned form, handwritten Chinese note) ≥ 80% pass on Qwen2.5-VL-7B

#### `xiaoguai-pdf-mcp` (v1.0)

- Rust MCP server, ~800 LOC
- Built on `pdfium-render` (Apache-2.0). Native Pdfium binary bundled.
- Tools:
  - `pdf_to_markdown(path, page_range?) → {markdown, page_count}`
  - `pdf_page_text(path, page_no) → {text}`
  - `pdf_page_render_to_image(path, page_no, dpi) → {png_bytes}` (handoff to vision-mcp for scanned pages)
  - `pdf_search(path, query) → {matches: [{page, snippet}]}`
- SQLite cache keyed by `(file_sha256, model_id, prompt_template_version)` — 30-50% hit rate on group-chat forwards
- Long-PDF strategy: chunked summary via map-reduce, never tries to stuff 200 pages in context
- Acceptance test: 50-PDF regression corpus (mix of text-extractable + scanned + tables), p95 latency <1s/page text, <8s/page OCR

#### `xiaoguai-asr-mcp` (v1.1)

- Rust MCP server, ~600 LOC
- Built on `whisper-cpp-plus-rs` (MIT) with Silero VAD pre-chunking + Whisper large-v3-q5 GGUF
- Tools:
  - `transcribe_audio(path, lang?) → {text, lang}`
  - `transcribe_with_timestamps(path, lang?) → {segments: [{start, end, text}]}`
- Streams partial transcripts to avoid blocking on long voice messages
- Cloud fallback: Volcengine ASR (Cantonese / heavy accent — Whisper CER ~5-7% Mandarin, ~15%+ Cantonese)
- DingTalk AMR legacy decoded via `symphonia` before Whisper
- Acceptance test: 20-clip Mandarin IM voice corpus CER <8%, p95 latency <0.5× realtime on RTX 4090

### IM adapter MIME router (lives in `xiaoguai-im-feishu`)

Single new code path in 飞书 adapter (mirrored for 钉钉/企微 in v1.1):

```rust
match message.mime_type() {
    "text/*" => forward_as_user_turn(text),
    "image/*" => {
        let result = mcp_call("xiaoguai-vision-mcp", "analyze_image", ...).await?;
        forward_as_user_turn(format!("[image-summary {}]", result.text))
    },
    "application/pdf" => { /* similar */ },
    "audio/*" => { /* similar */ },
    _ => unsupported_modality_reply(),
}
```

### Annotation format (locked design)

User-turn annotations from modality MCPs use bracketed `[key val ...]` notation, **not** raw JSON:

- ✅ `[image-summary content="Datastore latency >50ms on esx-prod-03" source_hash=abc123]`
- ❌ `{"type":"image_summary","content":"Datastore latency..."}` — JSON in user content pollutes context + confuses tool-call models

The bracketed form is human-readable and the agent treats it like a system annotation, not as user prose.

### Caching strategy

Common case: same image / PDF / voice message forwarded across multiple group chats. Cache key:

```
sha256(bytes) + model_id + prompt_template_version
```

Cache lives in `xiaoguai-pdf-mcp` SQLite (or per-modality equivalent). Eviction: LRU at 10GB cap.

Measured hit rate on internal Block / Anthropic IM data sources (per cited research): 30-50%.

### Roadmap split

| Version | In scope |
|---|---|
| v1.0 | image + PDF text-extractable (vision-mcp + pdf-mcp) |
| v1.1 | PDF scanned + audio (extend pdf-mcp `pdf_ocr_page` + add asr-mcp) + 钉钉/企微 adapters |
| v2.0 | TTS voice replies (`piper`), video frame-sample understanding, realtime streaming ASR (DingTalk 会议 / Feishu 妙记 webhooks), on-device VLM for air-gapped (Qwen2.5-VL-3B INT4) |

### Critical pitfalls captured

1. **vLLM Whisper 0.14.1 WER regression** (134% hallucination loop on L40S): pin vLLM 0.12.0 or use whisper.cpp. We default to whisper.cpp for predictability.
2. **Ollama vision detection flaky**: don't trust `/api/show`; declare `multimodal: true` in our own model registry table.
3. **Default Qwen tile-grid = ~1.5k tokens per 1080p screenshot**: ALWAYS downsample to ≤1280px max edge in vision-mcp before sending. Text-too-small after auto-downsample is the #1 production failure.
4. **Tesseract on OCR-Bench scores 34.4%** vs Qwen2.5-VL 73%. Don't reach for it as a "lightweight fallback" — it's worse, not lighter.

## Consequences

**Positive:**
- Core Rust binary stays pure-Rust + native-dep-free; `cargo install xiaoguai-cli` continues to work
- Each modality MCP can be upgraded independently (Pdfium release cycle, whisper.cpp release cycle, Qwen model release cycle)
- Process isolation means a Pdfium crash on a malformed PDF doesn't take down the whole agent platform
- Modality MCPs also work for **third-party agent platforms** (anyone can call our MCP servers) — community contribution direction
- Cache + downsample logic centralized in MCP server, not duplicated in agent loop or IM adapter

**Negative:**
- 3 extra packages to maintain (vision-mcp / pdf-mcp / asr-mcp) — each non-trivial
- Cross-process latency per modality call: ~50ms IPC overhead on stdio MCP transport vs in-process
- Operators need to manage modality MCP processes separately (helm chart adds 3 deployments)
- Modality MCPs need their own Docker images / installers — multi-binary release cadence

**Mitigations:**
- Modality MCPs share common base — `xiaoguai-mcp-base` crate with stdio transport, signal handling, telemetry hooks
- Helm chart bundles all three MCPs by default; can be disabled individually
- Container images small (base alpine + binary + Pdfium native or GGUF model)
- For low-resource customer environments, downgrade chart to `vision-mcp` only (PDF text via fallback prompt; no audio)

## Implementation

- **v0.5.6** Task 7 (in plan §9ter): scaffold `xiaoguai-mcp-base` shared crate
- **v1.0** new package: `xiaoguai-vision-mcp` + `xiaoguai-pdf-mcp` + IM adapter MIME router
- **v1.0** acceptance criteria: 6 IM scenarios from research note (Feishu vSphere alarm screenshot / DingTalk 30-page contract PDF / WeCom scanned customer form / Feishu voice query / DingTalk meeting recording / WeCom K-line screenshot)
- **v1.1**: `xiaoguai-asr-mcp` + `pdf_ocr_page` extension + 钉钉/企微 MIME routers
- **v2.0**: TTS + video + realtime streaming ASR

## References

- `docs/research/2026-05-21-local-agent-pain-points.md` §9ter I7
- BSWEN 2026-03 — Fastest PDF library for Rust benchmark
- vllm-project/vllm#33107 — Whisper large-v3 accuracy regression
- Roboflow blog — Best local VLMs for offline AI
- joshua8.ai — Dedicated OCR vs Vision LLMs vs Tesseract 2026
- jztan/pdf-mcp — chunked PDF reading reference impl
- ADR-0006 MCP Tasks primitive (used for long-running OCR / transcription)
