# Local Agent Pain Points — Consolidated Research

**Date:** 2026-05-21
**Sources:** 4 parallel research agents covering GitHub issues (7 projects, ~200 issues), Reddit + Hacker News, 知乎/V2EX/掘金/X/Twitter, long-form devblog reviews (~30 articles)
**Purpose:** Validate Xiaoguai's v0.5/v1.0 plan against real user pain. Identify what to add, drop, or reorder.

---

## 0. TL;DR — 7 findings ALL FOUR research streams agreed on

1. **MCP server lifecycle is structurally broken.** Silent disconnects (Claude Code #60428, qwen-code #4218), stdout contamination (TS SDK #1049), no health probe, no auto-restart. **Every** major client has this bug.
2. **"Thinking…" hangs are the #1 user-trust killer.** Long-running tools (`npm run dev`, vSphere clone, terraform apply) block the agent loop forever because MCP is synchronous + clients hard-code 10-15s timeouts. The MCP **Tasks primitive** (Nov 2025 spec) is the fix; no client has implemented it.
3. **Node/V8 2GB heap ceiling kills Node-based agents** (Cline #8868 heap OOM at ~10MB history; qwen-code #4167 Mark-Compact during compaction). Rust gives a structural moat *if* we bound per-session memory explicitly.
4. **Local-LLM tool calling is broken everywhere** — 7-13B models hallucinate tool names, return JSON-in-text instead of tool_calls, drop sequential calls (qwen-code #176, aider #5118, ollama #11135). Universal verdict: "local-first, not local-only" with cloud escalation for hard tasks.
5. **Token cost shock around week 2** — Cline ($50/day, rewrites whole files), OpenHands ($30/hour, oscillation), Cursor ($2000 in 6 months). **Diff-based edits vs whole-file rewrites is a 30-90% cost delta.**
6. **Hidden model downgrades destroy trust faster than any bug** — Cursor users repeatedly call this out by name; Anthropic April 23 postmortem is the same pattern (silent regression invisible to users).
7. **MCP audit / governance is the enterprise gap.** Only 44% of enterprises with AI agents have security policies. 5 MCP servers = 5 log schemas, no shared identity, no shared retention. **EU AI Act 2026-08-02** makes audit trails mandatory for high-risk systems.

---

## 1. Consolidated Pain Point Matrix

| # | Pain | Severity | Frequency | Cross-source | Xiaoguai response |
|---|---|:---:|:---:|:---:|---|
| 1 | "Thinking…" indefinite hang on tool/MCP calls | 🔴 | ⭐⭐⭐⭐⭐ | GH + HN + Reddit + Reviews | **MUST** — per-call deadline + circuit breaker + Tasks primitive (§3.1) |
| 2 | Node/V8 OOM in long sessions | 🔴 | ⭐⭐⭐⭐⭐ | GH + Reviews | **Validated** — Rust + bounded memory by design (§3.2) |
| 3 | Local-LLM tool-calling unreliable (Qwen3/Llama3 hallucinate) | 🔴 | ⭐⭐⭐⭐⭐ | GH + Reddit + Reviews | **MUST** — per-model dialect adapter layer (§3.3) |
| 4 | MCP silent disconnect (UI says Connected, runtime broken) | 🔴 | ⭐⭐⭐⭐⭐ | GH + HN + Reviews | **MUST** — Supervisor with runtime health probe (§3.4) |
| 5 | Context window mismanaged + silent compact | 🔴 | ⭐⭐⭐⭐ | GH + HN + Reviews | **MUST** — visible budget UI + explicit compact (§3.5) |
| 6 | Token cost catastrophe (diff vs whole-file) | 🔴 | ⭐⭐⭐⭐ | Reviews + HN | **MUST** — diff-only edits, never whole-file rewrites (§3.6) |
| 7 | MCP security: tool poisoning + command injection (5.5% / 43%) | 🔴 | ⭐⭐⭐⭐ | HN + Reddit | **MUST** — capability scoping + signed manifest (§3.7) |
| 8 | Streaming drops tool-call deltas (Ollama `/v1` swallows function calls) | 🟠 | ⭐⭐⭐⭐ | GH + Reddit | **MUST** — buffer-then-validate before dispatch (§3.8) |
| 9 | Hidden model downgrade (Cursor auto-mode, Claude weekly quota) | 🔴 | ⭐⭐⭐⭐ | Reviews | **MUST** — transparent routing UI, never silent fallback (§3.9) |
| 10 | No audit / governance / approval gate | 🟠 | ⭐⭐⭐ | All four | **MUST** — audit hmac chain + pre-tool approval (§3.10) |
| 11 | Crash recovery broken (lost sessions, lost kanban) | 🟠 | ⭐⭐⭐ | GH + Reviews | **MUST** — SQLite WAL session persistence (§3.11) |
| 12 | Git competence broken in agents (OpenHands pushes to default) | 🟠 | ⭐⭐⭐ | Reviews | **DO** — aider-style commit-per-change + never default-branch push |
| 13 | Tool-call ID invariant violations | 🟠 | ⭐⭐⭐ | GH | **DO** — encode in Rust type system |
| 14 | Setup/onboarding fails first hour | 🟠 | ⭐⭐⭐ | All four | **DO** — single binary, no `uvx`/Docker hard-deps |
| 15 | Dify too heavy (8 containers, 32GB minimum) | 🟠 | ⭐⭐⭐ | 知乎 + V2EX | **Validated** — single binary, 16GB target |
| 16 | Local model + Ollama defaults suboptimal (3 tok/s vs 21 tok/s after tuning) | 🟡 | ⭐⭐ | V2EX | **DO** — factory-tuned KV cache + MoE config |
| 17 | Tool-call piping (output of T1 → input of T2 without round-trip) | 🟡 | ⭐⭐⭐ | HN top-voted | **EXPLORE v1.1** — compose primitive in agent loop |
| 18 | Cursor self-hosted is hybrid (planning still in cloud) | 🟡 | ⭐⭐ | Reviews | **DO** — pure local OR explicit boundary |
| 19 | No secrets manager (creds leak into transcripts) | 🟠 | ⭐⭐⭐ | Reviews | **MUST** — secret never enters LLM context (§3.12) |
| 20 | Sub-agent results overflow parent context | 🟡 | ⭐⭐ | GH (claude-code #23463) | **DO** — explicit budget fork, results summarized |
| 21 | YAML config wall (goose) | 🟡 | ⭐⭐ | Reviews | **DO** — GUI/TUI + YAML, not YAML-only |
| 22 | TLS proxy kills `uvx` in enterprise | 🟡 | ⭐⭐ | GH (踩坑 #25) | **Validated** — Rust binary, never resolve PyPI at runtime |
| 23 | Frontend/CSS tasks uniformly bad (no vision) | 🟡 | ⭐⭐ | Reviews | **Out-of-scope** — defer, we're not chasing UI generation |
| 24 | Pricing opacity > pricing height (Cursor mystery limits) | 🔴 | ⭐⭐⭐⭐ | Reviews | **DO** — flat seat + visible meter |
| 25 | "Local-LLM-only" is a lie (3-5 tok/s on CPU = unusable) | 🟠 | ⭐⭐⭐ | Reddit + Reviews | **DO** — be honest, ship "local-first + explicit cloud escalation" |

---

## 2. Features users want that don't exist anywhere (or barely)

| # | Missing feature | Validation strength | Add to plan? |
|---|---|:---:|:---:|
| F1 | First-class **local-LLM dialect adapter** (per-model translator for Qwen3 / gpt-oss / Llama) | ⭐⭐⭐⭐⭐ | **v0.5.2** — new layer in xiaoguai-llm |
| F2 | **MCP Tasks primitive** (async + cancel + status for long-running tools) | ⭐⭐⭐⭐ | **v0.5.3** — first-class in xiaoguai-mcp |
| F3 | **MCP capability scoping + signed manifest** | ⭐⭐⭐⭐ | **v0.5.3** — already in plan; reinforce |
| F4 | **Tool-call piping** (`tool2.input.x = tool1.output.y` without LLM round-trip) | ⭐⭐⭐⭐ | **v1.1** — compose primitive |
| F5 | **Unified audit schema across MCP servers** + queryable | ⭐⭐⭐⭐ | **v0.5.1** — already in plan; reinforce |
| F6 | **Context budget UI** — live tokens by category, manual compact | ⭐⭐⭐⭐ | **v1.0** — chat-ui requirement |
| F7 | **Pre-tool approval gate** (smolagents #2213 etc.) | ⭐⭐⭐⭐⭐ | **v0.5.4** — bake into agent loop |
| F8 | **Crash-resistant session** (SQLite WAL turn-by-turn) | ⭐⭐⭐⭐ | **v0.5.1** — add to plan |
| F9 | **Transparent model routing** (UI says when fallback fires) | ⭐⭐⭐⭐ | **v0.5.2** + v1.0 UI |
| F10 | **Project-local memory file** (CLAUDE.md / QWEN.md equivalent) | ⭐⭐⭐ | **v0.5.4** — XIAOGUAI.md convention |
| F11 | **国密 SM2/SM3/SM4** support (信创/等保) | ⭐⭐⭐ (China only) | **v1.0** — compile flag |
| F12 | **WebSocket long-connection IM bridge** (no inbound IP) | ⭐⭐⭐ (China) | **v1.0** — Feishu/DingTalk/WeCom |

---

## 3. The "MUST add" specifications

### 3.1 Per-call deadline + circuit breaker + Tasks primitive

Every MCP tool invocation:
- Configurable deadline (default 30s, configurable per-tool)
- After deadline → emit "tool X timed out after Ns, [cancel / wait / background]" event to caller
- On 3 consecutive deadlines → circuit breaker opens for that tool for 5 minutes
- Long-running tools register as **MCP Task** (async with status: queued/running/done/failed) — agent doesn't block waiting; user gets status updates

### 3.2 Bounded memory by design

- Per-session history capped (configurable, default 200k tokens)
- MCP response size capped (configurable, default 1MB) — spill to disk + return pointer
- Tool result cache LRU-bounded
- `cargo bench` regression test fails build if a session can grow past cap

### 3.3 Local-LLM dialect adapter layer

New module `xiaoguai-llm/src/adapter/` with:
- `Qwen3Adapter` — parses JSON-in-text into tool_calls (handles `<tool_call>...</tool_call>` blocks)
- `GptOssAdapter` — handles gpt-oss tool calling quirks
- `Llama3Adapter` — handles Llama3 tool format
- `OpenAiAdapter` — passthrough for compliant models
- Auto-detect from `/api/tags` or model name; fallback chain

### 3.4 MCP Supervisor with runtime health probe

- Every 30s: MCP `ping` (or no-op tool call) — if no response in 5s, mark unhealthy
- Stdout/stderr split — never let MCP `console.log` contaminate JSON-RPC
- Auto-restart: 5s backoff, exponential, max 3 in 5 minutes → blacklist + alert
- **UI status reflects runtime, not last-known config** — fix the #4218 / #60428 class

### 3.5 Visible context budget UI + explicit compact

Chat UI shows:
- Current context: 12.3k / 200k tokens
- Breakdown: system 1k / messages 8k / tools 2k / pinned 1.3k
- Manual `/compact` button — confirm before destruction
- Auto-compact only as last-resort with warning + user can disable

### 3.6 Diff-based edits only

Tool call signature for file edits:
- `apply_diff(path, unified_diff)` — emits unified diff
- Internal validator: rejects diffs > 50% of file (anti pattern)
- Cursor/Cline cost catastrophe explicitly avoided by design

### 3.7 MCP capability scoping + signed manifest

MCP server manifest must declare:
```yaml
capabilities:
  filesystem:
    read: ["/workspace"]
    write: ["/workspace"]
  network:
    allow: []   # deny by default
  syscalls:
    profile: default-restricted
```
- Signed with cosign at publish
- Verified before spawn
- User capability prompt on first use (like browser extensions)

### 3.8 Streaming-safe tool-call parsing

- Buffer streaming chunks until complete tool_call JSON
- Validate against schema before dispatching
- Reject mid-call abort (Ollama `/v1` drops chunks); fall back to non-streaming for tool-call models

### 3.9 Transparent model routing

- Every response footer (or chat-ui side-panel): "Generated by `qwen2.5-coder-32b` via `local-ollama`, fallback to `claude-3.5-sonnet` via `anthropic` (2 times this session)"
- Never silent fallback — log + display every escalation

### 3.10 Audit hmac chain + pre-tool approval gate

Two-layer:
- **Audit chain** (already in v0.5.1): append-only, hmac-chained, every tool call logged with args + result hash
- **Approval gate**: configurable policy (per-tool, per-tenant, per-user). For destructive ops, agent emits "approval required for: drop_table(users)" → user clicks Approve in IM or web. Maps to 等保 + EU AI Act.

### 3.11 SQLite WAL session persistence

Per-turn write to session DB (separate from Postgres):
- Conversation state + tool calls + provisional results
- On restart, agent can resume mid-loop
- Eliminates "lost work" churn moment from Reviews

### 3.12 Secrets manager from day 1

- All secrets stored via `xiaoguai-secrets` (Vaultwarden or age-encrypted file)
- Never inlined into prompts
- Never logged
- Tools that need secrets receive a reference token; resolved server-side
- OpenHands' credential-leak class explicitly designed out

---

## 4. The "MUST NOT" list (negative patterns from competitors)

1. **Don't reinvent MCP transport** (Goose PDEATHSIG #9332 lesson)
2. **Don't ship a Node/Electron core** (V8 2GB ceiling kills qwen-code/Cline)
3. **Don't auto-compact silently** (Goose #9330 user backlash)
4. **Don't trust LLM output for destructive ops without dry-run + double-confirm**
5. **Don't make YAML the only config path** (goose mistake)
6. **Don't fetch from `uvx` at runtime** (踩坑 #25 corporate TLS)
7. **Don't return raw API blobs to the agent** (sanitize + truncate)
8. **Don't lock to one IDE** (Cody mistake)
9. **Don't make sub-agents inherit parent context implicitly**
10. **Don't ship a "free local tier" that doesn't actually work** (Cursor honeymoon end)
11. **Don't store transcripts unencrypted at rest** (等保 2.0 三级)
12. **Don't make MCP the only extension point** (some teams won't write MCP servers)

---

## 5. Plan adjustments

### v0.5.1 (Storage + Auth + RBAC) — additions

- **NEW Task 5b:** SQLite WAL session-persistence repository (parallel to PG). Session state survives core restart.
- **NEW Task 10b:** Audit "verify_chain" CLI command — admins can run `xiaoguai-cli audit verify --since 2026-05-01` and get integrity report.

### v0.5.2 (LLM Router) — major addition

- **NEW Task 1b: Local-LLM dialect adapter layer** (`Qwen3Adapter`, `GptOssAdapter`, `Llama3Adapter`). Auto-detect via `/api/tags`. **Critical** — addresses #3 highest pain.
- **NEW Task 8:** Transparent routing UI signal — every response includes provenance metadata `{model, provider, fallback_count}`.

### v0.5.3 (MCP) — major reinforcement

- **NEW Task 1c: MCP Tasks primitive** (async/cancel/status) — first-class, not opt-in.
- **NEW Task 4b: Stdout/stderr split + JSON-RPC stream integrity check** — refuses to dispatch if stream corrupted.
- **NEW Task 7b: Capability prompt UI** — first-use approval per (server, capability).
- **NEW Task 11: Per-MCP-call deadline + 3-strike circuit breaker.**

### v0.5.4 (Agent loop) — major addition

- **NEW Task 0: XIAOGUAI.md project-local memory** — auto-load if present.
- **NEW Task 2b: Pre-tool approval gate** (policy-driven, IM-deliverable).
- **NEW Task 4b: Diff-only file edits** — `apply_diff` tool primitive; reject whole-file rewrites.
- **NEW Task 7b: Bounded conversation history** — hard cap + warning UI at 80%.
- **CHANGE Task 6 (planning mode):** explicit user-confirmed compact, never silent.

### v1.0 — new requirements

- **C18 secrets-manager hardening** — promoted from "later" to "v1.0 blocker"
- **Context budget UI** (chat-ui requirement, not v1.1)
- **Transparent model routing UI** — fallback visible in chat
- **国密 SM2/SM3/SM4 compile flag** for 信创 customer variant
- **WebSocket-based IM bridge** (no inbound port required) — cc-connect-style architecture

### v1.1 → consider promoting if signal strong

- **Tool-call piping primitive** (HN top-voted) — promote v1.1 → v1.0 if customer pilot validates

### v1.0 → demote / drop

- **CodeAct mode** — research interesting (Apple +20% on small models) but adds Python sandbox surface area; defer to v1.1 unless customer asks
- **Workflow editor / DAG UI** — Dify/n8n have it; "too heavy" is a top complaint. Stay code-first + MCP-first.

---

## 6. Validation assumptions still open

| Assumption | Validation method |
|---|---|
| 国内政企客户愿为"统一审计"付费 | Interview 3 金融/央企客户 in v0.5 timeframe |
| Ollama 仍是国内本地 LLM 默认（vs vLLM/SGLang） | Survey 5-10 个真实用户 |
| 飞书是 v1.0 必含 IM (vs DingTalk first) | Customer signal — wait for design partner |
| EU AI Act 2026-08 触发海外审计需求 | Track regulatory deadlines, position v1.0 for compliance landings |

---

## 7. Key competitive intelligence summary

| Competitor | Their strength | Their fatal flaw | Xiaoguai positions as |
|---|---|---|---|
| **goose** | Free + 27k stars + MCP-first | YAML wall + hangs on long tools + no IDE | "goose without YAML, without hangs, with audit" |
| **aider** | git-native commit-per-change | Not really an agent (chat loop) + no MCP | "aider's git discipline + real agent + MCP" |
| **qwen-code** | OpenAI-compat + 2000/day free | Bad multi-file + V8 OOM + no compose | "qwen-code's protocol + Rust bounded memory + multi-file diff" |
| **Cline** | Best VSCode UX | $50/day token burn + whole-file rewrites | "Cline UX without token burn — diff-only" |
| **Continue.dev** | Most flexible config | "Core feature is mediocre" + lazy init | "Continue's flexibility + delivers on coding" |
| **Claude Code** | Best primitives + Skills | Cloud-only + context overflow + MCP hangs | "Claude Code primitives + local-first + bounded context" |
| **Cursor** | Best autocomplete | Hidden downgrades + opaque pricing + reverts work | "Cursor's UX + transparent pricing + no surprise" |
| **Cody** | Monorepo indexing | Enterprise-only $59/u/mo | "Sourcegraph indexing as MCP server, no platform lock-in" |
| **OpenHands** | Docker sandbox | Git broken + credentials leak + token spend | "OpenHands isolation + git competence + secrets manager" |
| **smolagents** | Tiny lib (~1k LOC) | No product (no UI, no tools) | "smolagents core philosophy + production-grade platform" |
| **Dify** | Visual workflow + plugins | 8 containers + heavy + SSRF defaults block LAN | "Dify's plugin breadth in 1 binary that allows LAN" |
| **FastGPT** | One-click + monolithic | Less plugin breadth | "FastGPT simplicity + MCP-first plugin breadth" |
| **Bisheng** | RAG + Chinese-native | Breaks at 71-doc scale + 32GB minimum | "Bisheng compliance angle + scalable RAG via MCP" |

---

## 8. Source links (all 4 research streams)

**Stream 1 — GitHub (200 issues across 7 projects):**
- cline #10853 / #8868 / #10631 / #10052
- claude-code #60866 / #60428 / #56437 / #23463 / #27431
- qwen-code #4167 / #176 / #1281 / #4218 / #4007 / #4008
- aider #3594 / #4737 / #5118 / #3199
- goose #9082 / #9332 / #9330 / #9118
- smolagents #2213 / #2117 / #2172 / #2176 / #2290
- OpenHands #14416 / #14323 / #14277 / #14348

**Stream 2 — Reddit + HN:**
- HN 43600192 ("S in MCP stands for Security")
- HN 46104557 (Tool calling broken without MCP composition)
- HN 45954572 (MCP wastes tokens vs regular tool calling)
- HN 45407016 (LLMs better at writing code than tool calling)
- HN 44560230 (UTCP — MCP without headaches)
- HN 47704729 (M×N problem of tool calling and open-source models)
- TrendMicro / RedHat / SecurityWeek / WorkOS / Maxim audit articles
- cc-connect (~27k stars, Feishu/DingTalk/WeCom bridge model)

**Stream 3 — 知乎 / V2EX / 掘金 / X:**
- zhuanlan.zhihu.com/p/1947389040702781389 (Dify 不看好)
- zhuanlan.zhihu.com/p/23031455330 (Dify 本地部署常见问题)
- 53ai.com/news/OpenSourceLLM/2025060235062 (Bisheng vs Dify)
- v2ex.com/t/1208365 (8GB 显卡跑 30B)
- zhuanlan.zhihu.com/p/2013706717012186762 (飞书/钉钉/企微集成)
- aliyun.com agentbay-security
- coder.com/blog/comparing-coder-agents-and-cursor-agents
- workos.com/blog/2026-mcp-roadmap-enterprise-readiness
- tonybai.com/2026/01/31 (Rust vs TypeScript in agent battleground)

**Stream 4 — Long-form reviews:**
- aitoolanalysis.com/goose-ai-review/
- blott.com (aider 4-week experience)
- qodo.ai (Cline vs Cursor)
- dev.to/maximsaplin (Continue.dev)
- ubicloud.com (AI Coding Sober Review)
- anthropic.com/engineering/april-23-postmortem
- medium.com/@mchechulin (OpenHands real-world)
- dredyson.com (Cursor 6 months)
- elite-ai-assisted-coding.dev (Qwen Code review)
- morphllm.com/comparisons/roo-code-vs-cline
- medium.com/@ai_transfer_lab (MCP timeouts fix)

---

## 9. Next actions

1. **Update plan docs**: `2026-05-21-v0.5-inner-loop.md` per §5 above (additions to sub-milestones).
2. **Add new ADRs**:
   - ADR-0002 — Bounded memory by design (#3.2)
   - ADR-0003 — Diff-only file edits (#3.6)
   - ADR-0004 — Transparent model routing (#3.9)
   - ADR-0005 — Local-LLM dialect adapter layer (#3.3)
3. **Run customer interviews** for assumption-validation table (§6).
4. **Update README** to make differentiation more concrete based on competitive matrix (§7).
