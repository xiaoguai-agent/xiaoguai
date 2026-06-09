# Implementation plan — Capability upgrade: the enterprise/offline governable agent platform

| | |
|---|---|
| Date | 2026-06-09 |
| Status | **Draft — awaiting owner review** (workflow: doc → tasks → review → execute) |
| Positioning | 企业级 / 离线的**可治理** agent 平台 — 凸显 audit + HotL + 纯离线;**不追云端、不做桌面端** |
| Triggered by | 对标 OpenClaw(虾) / Hermes(马) / 办公小浣熊·桌面端 2.0(熊) + 一张 AIOps multi-agent 总控图 |
| Hard constraints | DEC-033 unchanged: 单二进制 · 内嵌 SQLite · 单 owner · `:7600` · 不引 Postgres/Redis/外部队列/云依赖 |

## 0. Positioning & principles (owner-confirmed)

xiaoguai's lane is **"the agent you dare run autonomously inside an enterprise's
offline network, where every step is approved, audited, and reversible."** The
upgrade sharpens that lane — it does NOT chase the consumer "云端一体办公全家桶"
shape of 办公小浣熊.

**Differentiation we lean into (competitors mostly lack these):**
- **HMAC audit chain** — every action signed, chain-verified, compliance-exportable.
- **Full HotL governance** — suspend/resume, decision registry, redaction, timeout.
- **Pure offline** — single binary + SQLite + local LLM (Ollama). No cloud round-trip.
- **Governed rollback** — coding workspace checkpoint/rollback.

**Explicitly NOT doing (owner-confirmed):**
- ❌ **No cloud workbench / cloud-local hybrid.** 办公小浣熊's "本地+云端一体" is a
  *liability* in air-gapped/compliance deployments — staying pure-local is the moat.
- ❌ **No desktop client / global Quick Bar.** The existing **web UI (chat-ui +
  admin-ui) is sufficient**; we will not open a new desktop-shell frontend track.
- ❌ Nothing that breaks DEC-033 (no microservices, no cloud, no external queue).

## 0.1 Implementation philosophy — integrate-first (owner-confirmed)

**能用现成 skill/MCP 就集成,不自研.** xiaoguai's value is the **governance
(HotL + HMAC audit) + orchestration layer**, NOT re-building tools. Every
capability below is implemented by **integrating an existing MCP server / skill
and wrapping it in xiaoguai's governance**, not by writing a Rust tool crate —
because every tool call already passes `HotlGate::check(scope=tool_call.{name})`
+ audit in `react.rs`, so an integrated tool gets approval + audit-chain for free.

Self-build ONLY when (a) it IS the governance/orchestration itself, or (b) no
existing skill/MCP exists. An external MCP server is a **runtime optional
dependency** (like git/gh/chromium) — it does NOT break the single binary
(DEC-033); offline deployments just pre-install/bundle the recommended server.
This is the standing rule, not a one-off — saves build effort + tokens + upkeep.

## 1. Competitive read (why this upgrade, scoped)

The "虾 → 马 → 熊" evolution line: **execution → +memory → +everything/low-friction**.

| Capability (from 熊 / AIOps) | xiaoguai today | Gap → task |
|---|---|---|
| Local file read/write, dir-scoped grant | ✅ coding workspace + path fence | add Office formats → T1 |
| AI file edit, one-click rollback | ✅ checkpoint/rollback (governance-grade) | — |
| Offline local model (Ollama) | ✅ default + air-gapped embedder | — |
| MCP ecosystem | ✅ two-way (consume + serve) | — |
| Scheduled tasks | ✅ scheduler (cron/webhook/file) | — |
| IM (Feishu/WeCom/DingTalk) | ✅ all three | — |
| Memory / identity | ✅ memory crate + USER.md + RAG | multi-source → T7 |
| Expert center + "expert team" | 🟡 personas + packs/agents (parts, no product UI) | productize → T3 |
| Skill library (office/research/email) | 🟡 packs + agent-authored skills | office skills → T1 |
| One-sentence browser control | ❌ none | **T2 (browser)** |
| Office report/PPT/write-back Excel | ❌ only typst PDF | **T1 (Office)** |
| Multi-agent orchestration (Executive→Personas→Pipeline) | 🟡 orchestrator triangle (parts) | productize → T4/T5/T6 |
| Event-driven self-healing loop | 🟡 scheduler+watch+coding (parts) | wire → T6 |
| Install-and-go (WPS-level) | 🟡 pip + init wizard | polish → T8 |
| HMAC audit + full HotL | ✅ **unique moat** | — |

**Read:** xiaoguai already covers most of 熊's *hard* functions; the gaps are
(a) a few capabilities — Office, browser, productized expert center — and
(b) turning orchestration parts into a product. We add those **on the governance
+ pure-offline base**, which is exactly what enterprise/compliance buyers need
and what the cloud-coupled competitors can't offer them.

## 2. Capability upgrade list

**P0 — close the most-felt capability gaps (reuse existing parts)**
- **A. Office skills** — Excel/Word/PPT read+write + "one sentence → report → PPT →
  write back to a cell". Pure-local Rust libs (e.g. calamine for xlsx, docx-rs,
  typst for PDF/slides); no cloud. → **T1**
- **B. Browser control** — governed, offline; especially valuable to drive
  interface-less intranet systems (RPA replacement). Per the existing decision draft
  `docs/plans/2026-06-08-browser-automation-distribution.md`. → **T2**
- **C. Expert center, productized** — personas + packs → "pick an expert / form an
  expert team / one-click run" with a selection panel in chat-ui/admin-ui. → **T3**

**P1 — absorb the AIOps orchestration paradigm (from the image)**
- **D. Executive routing + parallel multi-agent orchestration + result synthesis** —
  extend `xiaoguai-orchestrator` from triangle to intent-route → parallel personas →
  synthesize/conflict-resolve. → **T4**
- **E. Consult/execute split + Agent Bridge** — explicit "consult mode (read-only)"
  vs "execute mode (HotL-gated)"; Bridge = a semantic wrapper over the existing HotL
  gate. → **T5**
- **F. Event-driven self-healing loop** — scheduler (Monitor) → orchestrator
  (Analyst/root-cause) → coding/tools (Executor), fully audit + HotL gated;
  alert → incident → self-heal → report. → **T6**

**P2 — polish & memory**
- **G. Install-and-go** — tighten the existing pip/brew/deb path + first-run wizard
  toward "install like WPS". (No desktop shell.) → **T8**
- **H. Memory multi-source + import** — team glossary + unify local/IM/knowledge-base
  sources; optional import of external memory. → **T7**

## 3. Task breakdown

| # | Task | Reuse | Size | Depends | Notes |
|---|---|---|---|---|---|
| **T1** | Office skills — **integrate** an existing office MCP server (xlsx/docx/pptx/pdf read+write, report/PPT gen) + skill/pack wrap | MCP consume + packs | **S–M** | pick the server | NO self-built crate; pptx problem disappears (external tool supports it); HotL+audit auto |
| **T2** | Browser control (governed, offline) | browser decision draft + McpClient pattern | L | owner decides chromium distribution first | mirrors `xiaoguai-coding` |
| **T3** | Expert center productization (expert + team + UI) | personas + packs + admin/chat-ui | M | — | route by intent |
| **T4** | Executive routing + parallel orchestration + synthesis | xiaoguai-orchestrator | M | — | triangle → intent-route+parallel |
| **T5** | Consult/execute split + Agent Bridge | HotL gate + toolbox | M | T4 | read-only vs HotL-gated mode flag |
| **T6** | Event-driven self-healing pipeline | scheduler + orchestrator + coding | L | T4, T5 | Monitor→Analyst→Executor, audit+HotL |
| **T7** | Memory multi-source + import | memory + RAG | M | — | team glossary, multi-source |
| **T8** | Install-and-go polish (no desktop shell) | install chain + init wizard | S–M | — | first-run UX to "WPS-level" |

## 4. Sequencing

1. **First wave (fastest felt value, reuses ready parts):** **T1 (Office) + T3
   (expert center)** — closes the biggest "熊-vs-小怪" experience gap.
2. **Second wave (orchestration paradigm):** **T4 → T5 → T6** — the AIOps
   Executive/分层/self-heal model on the governance base.
3. **Third wave:** **T2 (browser)** once the chromium-distribution decision is made;
   **T7 (memory)** + **T8 (install polish)** as finishers.

Each task ships the xiaoguai way: design note (where it needs a DEC) → plan → review
→ TDD → PR with clippy `-D warnings` + nextest green → merge. Nothing regresses
DEC-033; clean-box boot (`serve` on fresh SQLite, `:7600`, `/healthz`) stays green.

## 5. Boundaries (what this plan does NOT add)

- No cloud workbench, no cloud-local hybrid, no SaaS surface.
- No desktop client / global hotkey shell — web UI is the frontend.
- No Postgres/Redis/external queue/tenant model (DEC-033 intact).
- Office/browser/skills are **local, governed, auditable** — or they don't ship.

## 6. Open decisions for the owner (before execution)

1. **Browser distribution (T2)** — answer the 4 decision points in
   `docs/plans/2026-06-08-browser-automation-distribution.md` (chromium posture,
   provisioning, CDP crate, MVP tool surface).
2. **Office MCP server (T1)** — pick which existing office MCP server to integrate
   (e.g. markitdown / pandoc-based / office-mcp) + an offline-bundle plan for it.
   No self-built office crate (per §0.1 integrate-first).
3. **Design-repo DECs** — T4/T5/T6 (orchestration paradigm) are architecture-level;
   confirm whether they go through `xiaoguai-agent-design` DECs first (per the
   doc-first workflow) before code.
