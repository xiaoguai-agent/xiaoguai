# 收敛重构计划 (Consolidation Plan) — 2026-06-20

> 状态:**待 review**(doc-first,review 通过后才动代码)
> 作者:Claude(逐 crate 深审 + grep 校验后产出)
> 触发:功能靠 /loop sprint 一波波加上去,横切关注点被每个 feature 各写一遍 → "零散"。

---

## 0. 一句话结论

**不需要推倒重来。** 一次快扫 + 8 个逐簇深审独立收敛到同一句话:**宏观架构是对的,crate 边界是对的,存储层是干净的**(migrations `0001–0038` 无缺号、repository 模式一致)。债务几乎全部集中在 **5 类内部一致性问题**,且都能用"绞杀式增量"逐 PR 消除,产品全程可发版。

DEC-033 四条硬约束(单二进制 / 内嵌 SQLite / 单 owner / `:7600`)**不动**。本计划不涉及任何 schema / 协议 / 端点的破坏性变更(F 阶段的孤儿决策除外,且需 owner 拍板)。

---

## 1. 根因(为什么会"零散")

不是架构错,是**缺少一条被强制执行的横切约定**:没有规定错误处理 / 校验 / 审计 / 加载态"只能放在哪",于是每个 sprint 都重新发明一遍。
→ 真正的止血是 **DEC-041(§3)+ CI guard**,否则清理只是治标,下个 sprint 照样散。

---

## 2. 已校验的债务清单(5 类)

> 文件行数为实测 `wc -l`;"误报"项见 §7 校验日志,**不要**当 bug 再提。

### Bucket 1 — 横切关注点 per-site 重复(**最高杠杆**)
跨后端 / 前端 / CLI 的同一主题:

| 处 | 表现 | 证据 |
|---|---|---|
| 后端错误信封 | **4 种写法**(`ApiError` enum / 局部 `err_response` / 重定义 `struct ApiError` / 内联 `json!`),信封形状 `{code,message}` vs `{error}` 不一致 → 客户端解析脆弱 | `routes/{incidents,memory,personas,teams,providers,hotl_decisions}.rs`(见 §8 路由矩阵) |
| 后端审计 | best-effort append 在 ~5 个 handler 复制;**审计盲区**:`hotl_decisions/experts/outcomes/watchers` 改状态却无审计 | `routes/{memory,incidents,teams,admin,orchestrate}.rs` |
| 后端校验 | 仅内联 `is_empty()` 检查,无共享 validator | `routes/{sessions,loops,orchestrate,teams}.rs` |
| 后端 HotL gating | 多个 gate 类型/命名(`EnforcerGate`/`SuspendingHotlGate`/`HotlEnforcer`),per-handler 应用而非 layer;`EnforcerGate` 其实是 gate 不是 enforcer,命名误导 | `xiaoguai-core/src/hotl_bridge.rs:407` |
| CLI | `require_ok()` **复制 5 份**;5 个 wave-3 命令 HTTP-dispatch 形状相同零复用;表格输出各写各的 | `commands/{hotl,anomaly,outcomes,skills,watch}.rs` |
| CLI | `load_settings` 两份实现(cli wrapper vs `xiaoguai_core::load_settings`)同一二进制 | `cli/src/main.rs` |
| 前端错误 UX | **3 种**(i18n+`role=alert` / 裸 `<div class=error>` / 无),~10 个 pane 静默失败或缺 `role=alert`(a11y+i18n 缺口) | `admin-ui/src/panes/*`(见 §9 矩阵) |
| 前端加载态 | **3 种** shape(state-machine / null-check / boolean 三件套),无 `useAsyncState` | 同上 |
| LLM provider | `build.rs` 巨型 match + 每 provider 自写 `new()`;`resolve_required_key`/`resolve_optional_key` 近重复 | `xiaoguai-llm/src/build.rs` |

### Bucket 2 — IM 适配器系统性重复(独立大目标,边界干净)
7 个 IM 适配器 ~10.6 KLOC,**~245 LoC 系统性重复**:`ReplySink` enum(Stub/Recording/Api)×6、`now_unix()`+`TIMESTAMP_TOLERANCE_SECS=300`+constant-time 签名校验 ×5、时间戳新鲜度(SEC-05)×4、token-cache+OpenAPI client ×4。`ImProvider` trait 之外无共享基座。

### Bucket 3 — God file(机械拆分,多为低风险)
- `cli/src/main.rs` **2897**(clap 定义 1140 + dispatch + 内联 handler)
- `core/src/lib.rs` **1555**(boot 编排)、`hotl_bridge.rs` **1569**(4 个 trait impl)、`scheduler_bridge.rs` **1262**(8 个 adapter)
- `tasks/src/skill_author.rs` **1221** → validation/proposal/fixtures
- `llm/src/bedrock.rs` **1108**(auth/stream/parse)、`reranker.rs` **1167**(4 provider → 各成模块)
- `api`:incidents 散在 3 文件,且 `incidents.rs` **888** 把 token-gated ingest + owner-authed CRUD + pipeline 混在一起 → **按鉴权边界拆**;`providers.rs` 661
- `scheduler/src/runner.rs` **1058**(`fire()` 135 行 8 关注点)
- `audit/src/export.rs` **828**(SOC2/GDPR/HIPAA → 各成模块)
- `im-wecom/src/lib.rs` **802**、`im-dingtalk` 668
- 前端:`shared/src/index.ts` **2886**(130+ 导出 / 25 域 → 拆 wire.ts/client.ts/各域)、`chat-ui/ChatPage.tsx` **1029**(15+ useState → 抽 hook)
- ⛔ **`runtime/src/resilience.rs` 819 是"好的大文件"(单一职责、内聚),明确不拆。**

### Bucket 4 — 孤儿 / 半成品功能(字面意义的"零散":造了没接)
- ✅ **Anomaly(已校验)**:lib(9 文件)+ admin 面板(726 行 tuning UI)+ shared 类型(标 `v1.4 planned`)全在,但 **api/src 里 0 条 `/v1/anomaly/*` 路由**;`anomaly_detections_total` 指标永不触发;前端 pane 调用不存在的端点。→ **需 owner 决策(§6):接上 vs 隐藏 UI 并标 future。** 这是最干净的"散落半成品"样本。
- ⚠️ 待 impl 时再核实(低风险):`core/packs.rs::register_pack()` 疑似不可达 + pack→anomaly "F2" 接线 TODO;`memory` 的 `local`/ONNX `LocalEmbedder` = 不可用的 stub feature → 删或补;`mcp/servers/mod.rs` 只有 github_pr(可扩展但空);`worker_agent` 的 `tool_allowlist` 死字段(S9-5 占位,带 `#[allow(dead_code)]`)。

### Bucket 5 — 契约 / 接线正确性(小而精)
- HotL `DecisionRegistry` 必须是 gate 与 AppState 同一个 `Arc`(#284 历史)→ 加 `Arc::ptr_eq` 单测防回归。
- Qdrant `RagClient::query` 夹带 base64 向量(违反 text 契约);`reindex_path` 在 Tantivy/Qdrant 静默 `Unsupported`。
- `xiaoguai-types` 两个 `Role` enum 通配导入冲突 → 改名 `MessageRole`。
- chat-ui 无 i18n parity 测试(admin-ui 有);`ChatPage` 硬编码英文错误串;`Skills.tsx` 用裸 `fetch` + `as unknown` 绕过 client。
- scheduler 模型路由在 boot 时定死,provider 注册表运行时变更不生效。

---

## 3. DEC-041(本计划的基石,止血用)

> **提案:横切关注点单点化。** review 通过后录入设计仓(参照 DEC-040 流程)。

1. **后端错误**:全仓只有一个 `xiaoguai-api::error::ApiError` + 一份 `IntoResponse`;信封统一 `{code,message}`。路由里**禁止**出现 `StatusCode::` 映射或重定义 `struct ApiError`。
2. **后端横切**(审计 / owner-auth / 校验 / HotL gating):**axum middleware + extractor**,不得 per-handler 复制。
3. **前端**:数据请求统一走 `useAsyncState` hook;错误统一 `<ErrorBanner role="alert">`(i18n)。禁止裸 `<div className="error">`。
4. **CLI**:HTTP 错误走 `http_util::require_ok`;表格走 `output::Table`;wave-3 命令走 `ApiCommand` trait。
5. **CI guard**(随对应抽象一起落地):grep 级 lint —— 路由文件出现 `StatusCode::`/`struct ApiError`、pane 出现裸 error div、CLI 出现第二个 `require_ok` 即 fail。

---

## 4. 方法原则(让重构提质而非引入回归)

1. **绞杀式增量,不 big-bang**:每 PR = 抽一个共享抽象 + 迁移 N 个调用点 + 验证 + 单独 merge。
2. **改动前先补 characterization 测试**锁现有行为(尤其错误处理 / HotL / IM 签名——动了最危险)。
3. **一个 PR 只收敛一个横切关注点**,不和 god-file 拆分混。
4. **按 `杠杆 × 低风险` 排序**:删重复最多、最机械、最好测的先上。
5. **不发明新模式**:选现有最好的升级成共享抽象(如 Scheduler pane 的 state-machine),其余迁过去。
6. **每行改动可追溯到本文档的某条**(满足仓库工作流)。

---

## 5. 分阶段计划(每阶段独立可发版)

| 阶段 | 内容 | 杠杆 | 风险 | 预计 PR | 验证点 |
|---|---|---|---|---|---|
| **0 前置** | 录入 DEC-041;为将重构的横切路径补 characterization 测试(错误响应 / HotL / IM 签名) | — | 低 | 1 | 新测试全绿、锁住现状 |
| **A 后端错误统一** | 单 `ApiError`+`IntoResponse`,删 ~18 路由的局部信封;加 CI guard #1 | 高 | 低 | 1–2 | api 22 测试绿 + 信封一致性测试 + 路由矩阵全 `ApiError` |
| **B 前端 async/error 原语** | `useAsyncState`+`<ErrorBanner>`,迁 ~20 pane;chat-ui i18n parity 测试;`ChatPage` 去硬编码;`Skills.tsx` 改用 client | 高 | 低 | 2 | admin-ui 278 测试 + parity 测试 + e2e 绿 |
| **C 后端横切→layer** | 审计/owner-auth/校验/HotL gating → middleware+extractor;补审计盲区;`DecisionRegistry` ptr_eq 测试 | 高 | **中** | 2–3 | hotl_decisions/audit 测试 + P0 characterization 测试 |
| **D IM common crate** | 新 `xiaoguai-im-common`(ReplySink/now_unix/TimestampValidator/签名/`HttpReplyClient<T>`),迁 7 适配器 | 高 | **中**(安全敏感) | 2 | 每适配器签名/重放测试(SEC-05 必保)+ 共享测试 |
| **E god-file 拆分** | 后端 main/lib/bridges/skill_author/bedrock/reranker/incidents(按鉴权边界)/runner/export/im-wecom;前端 index.ts 分域 + ChatPage 抽 hook。**不动 resilience.rs** | 中 | 低 | 3–5 | 每拆一处:编译 + 全量测试,纯重构零行为变更 |
| **F 孤儿/契约清理** | anomaly(§6 决策后执行);删/补 ONNX stub、register_pack、mcp 空 seam、worker_agent 死字段;Qdrant 契约、`Role` 改名、scheduler 活路由 | 低 | 低–中 | 2–3 | 逐项 |

**A、B 在 0 之后可立即并行**(不同语言无冲突),是零回归风险的最佳起点。**C、D 是真正改善"质量"的核心,但必须先有 P0 测试网。** 总计约 **13–18 个 PR**。

---

## 6. 需要 owner 拍板的决策

1. **Anomaly 半成品怎么办?**
   (a) 补全 API surface(`/v1/anomaly/*` 路由 + scheduler 触发 + 接 lib)让面板真正可用;或
   (b) 把 admin 面板隐藏在"尚未开放"banner 后,lib 标 future、移除死指标。
   → 影响 F 阶段工作量与 anomaly 是否成为一个真功能。
2. **本轮收敛范围**:是否一次走完 0→F,还是先做 0+A+B+E(安全高价值核心)、C/D/F 视情况续?
3. **IM-common crate**:DEC-033 是"单二进制不拆微服务",**新增内部 lib crate 不违反**(workspace 内,仍编进一个二进制)——确认可加。

---

## 7. 校验日志(grep 实证,**勿当 bug 重提**)

深审 agent 的若干"孤儿/未接线"断言经 grep 证伪:
- ❌ Incidents 面板只读 → 实际 `Incidents.tsx` 调 `createIncident`+`dismissIncident`(DEC-040)。
- ❌ CLI `cli_config.rs`/`style.rs` 死代码 → main.rs 大量调用(`/config` + ANSI 主题 + diff 着色)。
- ❌ persona 未注入 agent → `orchestrate.rs` 经 `build_system_messages`+`filter_tools` 注入。
- ✅ anomaly 无 HTTP surface → 属实,见 Bucket 4。

教训:结构/重复类发现(与 `wc -l` 实测一致、多 agent 互证)可信;"未接线/孤儿"类断言**逐条 grep 后**才采信。

---

## 8. 附:后端路由一致性矩阵(节选,A 阶段基线)

| 模块 | 错误风格 | 校验 | 审计 | 鉴权 |
|---|---|---|---|---|
| admin / audit_exports | ApiError | inline | per-handler | middleware |
| incidents | 局部 `err_response` | inline | per-handler | middleware + ingest token |
| memory | 局部 helpers(信封 `{error}`) | inline | 半集中 | middleware |
| personas / teams | 重定义 `struct ApiError` | inline/shared | teams per-handler | middleware |
| providers | 局部 `bad_request/server_error` | inline | 无 | middleware(/v1 外) |
| hotl_decisions | `err_response`+ApiError 混用 | 无 | **无(盲区)** | middleware |
| experts / watchers | 无(unavailable 直接 panic) | 无 | **无(盲区)** | middleware |
| sessions / loops / usage / outcomes / mcp | ApiError | inline/无 | 多数无 | middleware/Claims |

目标:A 阶段后此列全部为 `ApiError`,审计列无盲区(C 阶段)。

## 9. 附:前端 pane 一致性(汇总)
- 加载态:5 state-machine(最佳)/ 9 null-check / 6 boolean 三件套。
- 错误显示:10 `role=alert`+i18n(达标)/ 10 裸 div / 5 无。
- `PaneIntro`:14/18 有;缺 Eval / Marketplace / Outcomes / Scheduler。
- 目标:B 阶段后统一 `useAsyncState` + `<ErrorBanner>` + `PaneIntro`。
