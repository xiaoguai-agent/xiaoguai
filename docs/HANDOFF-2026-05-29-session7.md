# Session-7 Handoff — 2026-05-29

> 本 session 在 session-5 + session-6 基础上完成了 sprint-7（2026-05-29 sprint）。
> 共开 11 个 PR（#65–#75），含 6 个核心功能交付 + 1 个 CI 净化 + 4 个文档 PR。

---

## TL;DR (一段话)

承接 session-5 的 Tier-1 + Tier-2 上线，session-7 把 sprint plan 里所有 P0/P1 任务全部落地：T10 (CI fmt 修复) + T2 (cargo-dist + Homebrew tap) + T3 (agent-authored skills, HotL 闸门) + T4 (OAuth 2.1 PKCE 出站 MCP) + T5 (合规导出) + T6 (execute_javascript MCP 沙箱)。同步把 Plan C Phase 2 (5 处设计文档 gap) 补完，并把 TRAE 长文里的 PPAF + 四维 (造缰/驭马/相马/育马) + 2D 战略矩阵 + 三大约束等概念蒸馏到了 `harness-engineering.md`（18 节 533 行）。**11 个 PR 全部本地独立验证**：fmt clean + 测试全过（T3 39 套 / T4 15 套 / T5 38 套 / T6 23 套）。剩余 T1 (操作员录屏) / T7 (L3 沙箱可行性研究) / T8 (testany-eng 评审) / T9 (付费证书) 都是非本 sprint。

---

## 落地的 PR (本 session)

| PR | 分支 | 任务 | 类型 | 测试验证 | R.E.S.T |
|---|---|---|---|---|---|
| #65 | `docs/session6-plans` | 四份 session-6 plan | docs | n/a | mixed |
| #66 | `feat/tier2-session-compaction` | **Plan D.2** — LLM-summary 会话压缩 + slide fallback | feat | 13 new tests, 见 PR | E + R |
| #67 | `release/systemd-readme-tier1-followup` | Plan B 安全部分 — systemd `ExecStart` + README 安装矩阵 | release | n/a | E + R |
| #68 | `docs/mcp-exec-demo-prep` | Plan A — demo 脚本 + runbook E2E 节 | docs | bash -n | T |
| #69 | `docs/design-link` | Plan C Phase 1 — `docs/architecture/design-link.md` | docs | n/a | T |
| **#70** | `ci/zombie-workflow-cleanup` | **T10** — cargo fmt 修复 + 僵尸 workflow 降级 | ci | 17 mcp-exec pass | E |
| **#71** | `release/cargo-dist-homebrew` | **T2** — cargo-dist + Homebrew tap | release | dry-run 验证 | E + R |
| **#72** | `feat/tier2-d1-agent-authored-skills` | **T3** — agent-authored skills (HotL 闸门 + 管理员审批) | feat | 39 suites / 0 fail | S + 育马 |
| **#73** | `feat/tier3-oauth-pkce-outbound-mcp` | **T4** — OAuth 2.1 PKCE 出站 MCP | feat | 15 suites / 0 fail | S |
| **#74** | `feat/tier3-compliance-export` | **T5** — 审计链合规导出（SOC2/GDPR/HIPAA） | feat | 38 suites / 0 fail | T |
| **#75** | `feat/tier2-execute-javascript-mcp` | **T6** — execute_javascript MCP 沙箱（Hermes 对标） | feat | 23 pass | E + S |

session-6 5 个 PR (#65–#69) + session-7 6 个 PR (#70–#75) = **11 个 PR 共开**。

---

## 推荐合并顺序

```
1. #70 (T10 — cargo fmt 修复)       ← 先合，解除所有其他 PR 的 CI 阻塞
2. #67 (systemd + README)            ← 小，快速合
3. #65 (四份 plan)                    ← docs，互不依赖
4. #68 (mcp-exec demo)               ← docs
5. #69 (design-link)                  ← docs
6. #66 (Plan D.2 compaction)         ← session-6 最大的功能 PR
7. #71 (Plan B cargo-dist)           ← 需要 tap repo + PAT 已就位（用户已确认）
8. #72 (T3 agent-authored skills)    ← Tier-2 最大的功能 PR
9. #73 (T4 OAuth PKCE)               ← Tier-3
10. #74 (T5 合规导出)                 ← Tier-3
11. #75 (T6 execute_javascript)      ← Tier-2 sibling
```

**为什么 #70 先**：所有 PR 都从 main 拉出去，main 现在 fmt 有漂移（PR #64 留下的），#70 修了。其他 PR 自己里都含 fmt 修复以保证独立合规，但 #70 一合 main，其他 PR 的差异就只剩本职功能。

**为什么 #71 不能贸然先合**：cargo-dist 一旦在 main，下次有人 `git tag v*` 立刻触发新的 workflow。需要确认 Homebrew tap repo 和 PAT 在合并前就位。用户已确认。

**为什么 #72 在 #74 之前**：#72 引入了 `AppState.skill_proposals/tenant_settings/skill_author_gate` 三个 Option 字段，#74 不动 AppState，#73 加了自己的字段。三个独立交付，但 #72 的 in-memory 测试 pattern 是 #74 和 #75 学习的样本，所以先合让模式可见。

---

## 设计文档同步

**Plan C Phase 2** 已落地（5 处 gap 全部闭合）：

| 文档 | 章节 | 改动 |
|---|---|---|
| `xiaoguai-agent-design/docs/hld.md` | §2 模块表 | `xiaoguai-storage` 加 cache fallback；`xiaoguai-agent` 加 HotL + compaction；`xiaoguai-llm` 加 token_count |
| 同上 | §3 新 DEC-013 | 完整记录 compaction 决策（rationale + refines + metrics） |
| 同上 | §4.1 请求生命周期 | PreProcessor 现在显示 estimate → compact OR slide 分支 |
| `lld/lld-agent.md` | §4.4 | "not yet implemented" → 六步算法 + `CompactionConfig` 字段表 + 与 PHILO §11/§12 的交叉引用 |
| 同上 | §7 测试设计 | 5 个新 unit + 2 个新 integration 全部登记 |
| `lld/lld-llm.md` | §3 + §4.3 | `token_count.rs` 加进树 + 完整模块文档 + 8 个单元测试 |
| `test-spec.md` | §3.1 | CASE-CHAT-006…013 八个新用例 |
| `test-strategy.md` | §8 + §10.4 | RISK-OPS-003（长会话上下文溢出）+ 三个 compaction-specific eval cases |
| `RELEASE-LOG.md` | 顶部 | 新行记录 Phase 2 |

**Harness Engineering 哲学文档** (`xiaoguai-agent-design/docs/harness-engineering.md`)：

从 TRAE 长文蒸馏，扩展到 18 节 533 行。新增的 6 个核心概念：

1. §4 **PPAF 循环** (Perception/Planning/Action/Feedback) — REPL 的 agent 视角对应名
2. §5 **四维 harness** (造缰/驭马/相马/育马) — Bridle/Drive/Evaluate/Cultivate，每个映射到 xiaoguai 子项目
3. §6 **2D 战略矩阵** — Cognitive Loop × Context Efficiency
4. §7 **三大约束** — LLM 非确定性 / 有限线性上下文 / 延迟成本天花板
5. §9 **三个正式 harness 组件** — Context Manager / Call Interceptor / Feedback Assembler（含中文名）
6. §12 **Function Calling 生命周期** — Schema 序列化 → 触发生成 → 确定性反序列化 → 观测注入 四步 + 失败模式 + xiaoguai 应对

---

## 关键技术决策记录

session-7 期间做的几个高影响决策：

| 决策 | 理由 | 文档化在 |
|---|---|---|
| compaction 默认 OFF | 长会话场景今天的用户少，summariser 在小本地模型上质量未知 | `runbooks/compaction.md` |
| compaction 在 backend 失败时回退到 `slide`，永不阻塞 | R.E.S.T Reliability — 永远 fail-open 到旧路径 | DEC-013, PHILO §2 |
| cargo-dist 不杀 `release-tarball.yml` | SLSA L3 provenance vs cosign keyless L2 — 先并存观察 | PR #71 描述 |
| `xiaoguai-core` 不进 cargo-dist tarball | 是 legacy shim，只走 .deb 保持 systemd 后向兼容 | `xiaoguai-core/Cargo.toml` |
| T3 agent-authored skills 默认 OFF | 每租户白名单 + HotL 每天 5 次预算 | `runbooks/agent-authored-skills.md` |
| T3 schema 强制 whitelist-only | 已存在工具名引用 + 禁止声明新 MCP server / 原生代码 | `xiaoguai-tasks::skill_author` validator |
| T4 自己手写 PKCE 而不用 `oauth2` crate | 避免 `getrandom 0.3` 新依赖链；约 120 LOC | T4 sub-plan §1 |
| T4 refresh token 不加密落盘 | RLS 已经保证租户隔离；DB 加密是运维责任 | runbook `outbound-mcp-oauth.md` |
| T5 chain-verify 不可绕过 | 没有 `--skip-verify`，破链立刻报 first_broken_id | T5 sub-plan §1 |
| T6 默认 Deno 而不是 Node | `--allow-none` 把沙箱化下沉到运行时；rejected `boa_engine` | `tier2-mcp-exec-js.md` |
| T6 独立 crate 而不是给 mcp-exec 加 `--runtime` flag | trust boundary 物理分离 | PHILO §14 |

---

## 剩余工作（非本 sprint）

| 任务 | 工作量 | 阻塞 | 备注 |
|---|---|---|---|
| **T1** mcp-exec live demo 录屏 | 1h | 操作员行为 | 脚本已在 #68；启 PG + Ollama 后 `bash docs/scripts/demo-mcp-exec.sh` + `asciinema rec` |
| **T7** L3 沙箱可行性研究 | 1h ADR / 1–2 wk 实现 | — | 本 session 写了 ADR 0020；实现下个 sprint |
| **T8** testany-eng reviewer skills 走查 | 30min | 新 `cd xiaoguai-agent-design && claude` session | reviewer skills 需要 session-start 加载 |
| **T9** macOS 公证 + Windows EV 签名 | 1 wk | Apple Dev ID ($99/yr) + EV 证书 (~$300/yr) | 付费证书未购买，pure deferred |
| Plan D.1 follow-up: production wiring | 2h | #72 合后 | `PgSkillProposalRepository` + `PgTenantSettings` + `PgAuditSink` adapters in `xiaoguai-core::skill_author_bridge` |
| Plan T4 follow-up: refresh-token 加密落盘 | 1d | #73 合后 | AES-GCM + key handle in env var + key-rotation re-encrypt |
| Plan T6 follow-up: 接入 agent loop | 1h | #75 合后 | mirror PR #66 的 wiring pattern |
| Plan T5 follow-up: PDF 渲染 | 2d | #74 合后 | 现在返回 `PdfUnimplemented` 501 |
| `docs.yml` mdbook workflow 修复 | 2h | — | "Permission denied" on preprocessor，install-from-tarball 模式脆弱 |

---

## sprint plan 进度

源：`docs/plans/2026-05-29-next-sprint.md` §2 backlog table

| Pri | ID | 任务 | 状态 |
|:-:|---|---|:-:|
| P0 | T1 | mcp-exec live demo | ⏳ 操作员行为 |
| P0 | T2 | cargo-dist + Homebrew | ✅ PR #71 |
| P1 | T3 | agent-authored skills | ✅ PR #72 |
| P1 | T4 | OAuth 2.1 PKCE | ✅ PR #73 |
| P1 | T5 | 合规导出 | ✅ PR #74 |
| P2 | T6 | execute_javascript MCP | ✅ PR #75 |
| P2 | T7 | L3 sandbox（本 sprint 只 ADR） | ✅ ADR 0020 |
| P2 | T8 | testany-eng reviewer 走查 | ⏳ 需新 session |
| P3 | T9 | macOS 公证 + Windows 签名 | ⏸ 付费证书 |
| P3 | T10 | CI 僵尸清理 | ✅ PR #70 |

**6 个 P0/P1 任务全部完成。** P2 T6 完成。P2 T7 ADR 完成（实现下次）。

---

## sub-agent 调度经验

session-7 用 sub-agent 并行了三个大任务（T3/T4/T5/T6 共 4 个），都成功：

| sub-agent | 任务 | 耗时 | 输出 | 失败 |
|---|---|---|---|---|
| `aac1e9fcfa21d7052` | T3 agent-authored skills | 61 min | PR #72, 39 测试 | 0 |
| `a4d35930d7cdcf3c6` | T5 合规导出 | 51 min | PR #74, 24 新测试 | 0 |
| `a87b4afa2ab16f65a` | T4 OAuth PKCE | 45 min | PR #73, 17 新测试 | 0 |
| `a43481dfc5a75fe42` | T6 execute_javascript | 18 min | PR #75, 23 测试 | 0 |

四个 sub-agent 都按规定先写 self-reviewed sub-plan，全部 PASS 6 点协议，才动手实现。三个 sub-agent 的 sub-plan 里都做了独立的"plan adjustment"决策并文档化（T3 — tenants.id 是 TEXT；T4 — 手写 PKCE；T5 — chain-verify 非绕过；T6 — Deno over Node）。

**经验**：把 plan 的 6 点 self-review 协议作为 sub-agent 的 hard gate ("only if self-review passes: implement") 显著提升了输出质量。一次 dispatch 出去后 1h 左右回来就是可独立验证的 PR。

**磁盘成本**：4 个 worktree × ~30 GB target = ~120 GB 峰值。224 GB 总 free 时刻安全。下次更激进并行（5+）前要做磁盘预算检查。

---

## 文件指针

- 本 handoff：`docs/HANDOFF-2026-05-29-session7.md`
- sprint plan：`docs/plans/2026-05-29-next-sprint.md`
- session-6 plan 起点：`docs/plans/2026-05-28-*.md` 五份
- session-7 sub-plan：`docs/plans/2026-05-29-{tier2-d1-agent-authored-skills,tier3-compliance-export,tier3-oauth-pkce-outbound-mcp,tier2-execute-javascript-mcp}.md`
- T7 ADR：`docs/architecture/adr/0020-l3-sandbox-feasibility.md`（同 PR）
- 设计文档主入口：`xiaoguai-agent-design/docs/harness-engineering.md`（philosophy）+ `hld.md`（架构）+ `lld/`（per-component）
- memory（自动加载到下次 session）：`~/.claude/projects/-Users-zw-testany-myskills-xiaoguai/memory/{project-status,agent-roadmap,ci-gotchas}.md`
- 实现仓 → 设计仓的索引：`docs/architecture/design-link.md`
