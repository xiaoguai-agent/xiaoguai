# 安全审计报告 — xiaoguai

- **日期**：2026-06-10
- **分支**：`feat/t7-memory`
- **方法**：6 个并行只读审计 agent（Fable 模型），分维度扫描 + 真实 `file:line` 取证。
- **威胁模型**：单二进制 AI agent，监听 `:7600`，内嵌 SQLite，**单 owner（无多租户是 DEC-033 有意设计，不算缺陷）**。HTTP API + SSE + IM webhook 暴露在外；LLM 可调用工具/执行代码，最大风险链 = **prompt injection → 任意执行 → 数据外泄**。

> 说明：本报告仅审计，未改动任何代码。每条发现带稳定 ID 方便建 issue / 对照修复。跨维度重复的发现已合并（标注「另见」）。

---

## 摘要

| 严重度 | 数量 | 概要 |
|--------|------|------|
| 🔴 CRITICAL | 2 | 默认配置无认证暴露全部 API；HotL 审批门默认放行 |
| 🟠 HIGH | 3 | MCP 子进程泄漏宿主密钥；Gemini key 回显前端；DingTalk 签名可重放 |
| 🟡 MEDIUM | 9 | CORS 全放行、错误回显、SQLite 明文落盘、L1 沙箱不隔离、IM 无重放防护、CLI 审计 dev key 回退、密码存 sessionStorage 等 |
| 🟢 LOW | 9 | 无速率限制、路径遍历、git hook 执行、Telegram 跳过验签、redaction 覆盖不全、前端 href 无白名单、无 CSP 等 |

**最关键的结论**：闸门设计本身齐全（consult/execute 分离、HotL Suspend、audit、L3 wasm 真沙箱、coding 路径校验都在），但**默认配置下闸是开的**。安全完全依赖 owner 手动：配 auth + 配 HotL 策略 + 选 L3 后端 + 给第三方 MCP 做外层隔离。任一未做，攻击链即贯通。修复重点应是**把默认态从 fail-open 改成 fail-closed**。

---

## ✅ 修复状态（2026-06-10，分支 `fix/security-audit-2026-06-10`）

全部 26 条已修复（fail-safe 轻量方案：重型项用不破坏现有部署的保守实现）。验证：`cargo check --workspace --all-targets` 通过；改动涉及的全部 crate 单测/集成测试通过；两个前端 `tsc`+`vitest`+生产构建通过，构建产物 `index.html` 无内联脚本（验证 SEC-26 的 `script-src 'self'` 不破坏 UI）。

| ID | 状态 | 落地 |
|----|------|------|
| SEC-01 | ✅ | config 默认 `127.0.0.1`；core 在非回环+无 auth 时**拒启动**（opt-out env `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK`）；Dockerfile/compose/config 同步保留容器行为 |
| SEC-02 | ✅ | `SqliteHotlEnforcer` 空策略时高危 scope（`execute_*`/`edit_file`/`git_*`…）返回 `Escalate`→默认 `SuspendingHotlGate` 挂起等审批；read-only 仍 Allow；新增测试 |
| SEC-03 | ✅ | `StdioMcpClient::spawn` 先 `env_clear()`，仅注入白名单(PATH/HOME/TMPDIR/LANG…)+显式 envs；新增泄漏测试 |
| SEC-04 | ✅ | Gemini 改 `x-goog-api-key` header；所有 reqwest 错误经 `without_url()` 脱敏；新增 header 断言测试 |
| SEC-05 | ✅ | DingTalk 验签加时间戳新鲜度（±300s，毫秒）+ 测试 |
| SEC-06 | ✅ | `build_cors()` 替换 `permissive()`：默认仅反射回环 origin，`XIAOGUAI_CORS_ALLOWED_ORIGINS` 可显式放行 |
| SEC-07 | ✅ | providers/memory 的 5xx 改泛化 `"internal error"`，详情仅 `tracing::error!` |
| SEC-08 | ✅ | `db::connect` 创建后对 DB+`-wal`+`-shm` 设 `0o600`（cfg(unix)，失败仅 warn）+ 测试 |
| SEC-09 | ✅ | L1 未隔离后端构造时一次性 `warn`（`Once`/Atomic），`XIAOGUAI_MCP_EXEC_ACK_UNISOLATED` 可静音；默认行为不变（非破坏） |
| SEC-10 | ✅ | ulimit `-u 1024 && -f 4194304(2GiB) && exec`（强制、失败即拒），`-v` 尽力而为；命名常量 + 测试 |
| SEC-11 | ✅ | 无 owner auth 时对 `/v1`（含 approve-repair / hotl decisions）显著 `warn`；叠加 SEC-01 非回环拒启动 |
| SEC-12 | ✅ | Feishu/WeCom/Discord 验签加时间戳新鲜度窗口（时钟可注入）+ 测试 |
| SEC-13 | ✅ | gateway 新增 `EventDeduper`（`(provider,event_id)` 内存 TTL 去重，10min）+ 测试 |
| SEC-14 | ✅ | WeCom gettoken（GET-only）的 transport 错误经 `without_url()`，corp_secret 不再进日志 |
| SEC-15 | ✅ | CLI 审计 key fail-closed：缺真实 key 时拒写（不再 fallback 已公开的 dev key）；新增 `DEV_AUDIT_HMAC_KEY` 常量 |
| SEC-16 | ✅ | 密码改模块级内存（`credentials.ts`），不落 sessionStorage；刷新重登 + 测试 |
| SEC-17 | ✅ | `redact.rs` 补 `sk-`/`xox[baprs]-`/`gh[pousr]_`/`github_pat_`/`AIza`/URL-query 模式 + 测试 |
| SEC-18 | ✅ | auth 失败全局滑窗节流（前 5 次免延迟，之后指数延迟封顶 3s；只延迟不拒绝，不锁死 owner） |
| SEC-19 | ✅ | list 仅返回非密前缀 + `Cache-Control: no-store`；revoke 按前缀（歧义→Conflict），URL 不再含完整 secret + 测试。注：哈希落盘/id-revoke 需迁移（超轻量范围），当前已是非回归改进 |
| SEC-20 | ✅ | 所有 git 子进程加 `-c core.hooksPath=/dev/null`（禁工作区 hook 执行）+ 测试 |
| SEC-21 | ✅ | Telegram `verify_secret` 在 secret 为空/None 时拒绝（fail-closed）+ 测试 |
| SEC-22 | ✅ | `git_push` 拒绝以 `-` 开头的 remote/branch（选项注入）+ 测试 |
| SEC-23 | ✅ | eval `suite_name` 拒 `/`、`\`、`..`、绝对路径 |
| SEC-24 | ✅ | 新增 `safeHref()` 协议白名单（http/https/file/mailto/obsidian/r2r），citations 不安全则渲染纯文本 + 测试 |
| SEC-25 | ✅ | `safeHref()` 应用于 AiDisclosureBanner / Marketplace 链接 |
| SEC-26 | ✅ | core 加 CSP + `nosniff`/`X-Frame-Options: DENY`/`Referrer-Policy` 中间件（全响应）；vite 关闭 inline modulepreload polyfill，构建验证无内联脚本 |

**未在本轮改动（有意保留，见各项说明）**：at-rest 数据库字段级加密接线（SEC-08 仅做文件 0600）、强制 L3 wasm（SEC-09 仅警告）、HttpOnly cookie 会话重构（SEC-16 用内存方案）、webhook token 哈希落盘（SEC-19 用前缀脱敏）——均为 owner 选定的 fail-safe 轻量方案，避免破坏现有部署/前后端协议。

---

## 🔴 CRITICAL

### SEC-01 默认 `0.0.0.0` 上以无认证暴露全部 `/v1/**`
- **维度**：认证与 API
- **位置**：`crates/xiaoguai-config/src/lib.rs:456-468`、`crates/xiaoguai-core/src/lib.rs:1250-1256` / `:872`、`crates/xiaoguai-cli/src/main.rs:1082-1087`、`crates/xiaoguai-api/src/routes/mod.rs:279-286`
- **证据**：`Settings::default()` 为 `host: "0.0.0.0", port: 7600` 且 `auth.username/password` 为空。`xiaoguai serve` 无 `--config` 时走 `load_from_env()` 以此为基底。`build_auth` 在凭据为空时**仅 `tracing::warn!` 后返回 `None`**，于是 `router` 不挂载 `require_auth`，但 bind 仍用 `0.0.0.0`。
- **风险**：开箱即用的 `xiaoguai serve` 在所有网卡监听且零认证，整个 `/v1/**`（会话、记忆、orchestrate、incidents approve-repair、`/v1/admin/providers` 改写 provider/API key）对局域网/公网任意来访者开放。叠加 SEC-02，prompt-injection 之外还存在直接的未授权控制面。
- **修复**：默认 `host` 改 `127.0.0.1`；当 `host` 非回环且未启用 auth 时**拒绝启动**（fail-fast），而非告警继续 serve。

### SEC-02 HotL 审批门默认放行（空策略表 = 无条件允许）
- **维度**：执行与沙箱
- **位置**：`crates/xiaoguai-core/src/hotl_bridge.rs:225-228`
- **证据**：`SqliteHotlEnforcer` 在 `hotl_policies` 表为空时直接 `return Ok(HotlVerdict::Allow)`。迁移文件（`0011`/`0017`/`0026`/`0027`）只建表不插种子策略，策略种值靠 owner 手动按 runbook 配。即便配了策略，模型也是「按 scope 窗口计数/计费」，没有「此工具危险→每次都要批」的原语。生产 serve 路径确实接了 gate（`crates/xiaoguai-core/src/lib.rs:451` `with_hotl_gate`），但 gate 背后默认放行。
- **风险**：开箱即用（含 macOS 开发默认态）下 `execute_python` / `edit_file` / `git_push` 等危险工具**零人工 checkpoint** 直接执行。这与「可治理 agent / HotL 审批」的产品定位直接矛盾——HotL 实为 opt-in 限速器，而非默认安全门。
- **修复**：(1) 对高危 scope（`tool_call.execute_python`、`tool_call.edit_file`、egress 类）内置 fail-closed 默认策略，空表时走 `Escalate`/`Deny`；(2) 增加「always-approve」策略类型（`max_count=0` = 逐次审批），与计数预算解耦；(3) 启动时若高危工具已注册但无任何策略，显著告警。

---

## 🟠 HIGH

### SEC-03 MCP supervisor 启动子进程不清理环境变量，宿主密钥整体泄漏
- **维度**：执行与沙箱
- **位置**：`crates/xiaoguai-mcp/src/stdio.rs:52-56`（无 `env_clear()`），调用方 `crates/xiaoguai-mcp/src/supervisor.rs:183-188`
- **证据**：`tokio::process::Command` 默认继承父进程全部环境，代码只「追加」`envs` 中的键，既无 `env_clear()` 也无 `env_remove`。宿主密钥确实在 env 里：审计签名密钥（`core/src/lib.rs:268`）、`AWS_SECRET_ACCESS_KEY`（`llm/src/bedrock.rs:81`）、飞书/钉钉/企微 token+secret（`core/src/lib.rs:978-1097`）等。
- **风险**：任何 operator 注册的第三方 stdio MCP server 子进程可直接读到全部宿主密钥。**审计签名密钥泄漏尤其严重**——拿到即可伪造 HMAC 审计链，让 approved Executor / self-healing 的审计取证失效。这与 `xiaoguai-mcp-exec`、`xiaoguai-coding/git.rs` 已做的 env 白名单清理自相矛盾。
- **修复**：`StdioMcpClient::spawn` 先 `cmd.env_clear()`，再仅注入显式声明的键 + 最小集（`PATH`/locale），与 exec/git 子进程统一。

### SEC-04 Gemini API key 通过 SSE 错误事件回显给聊天客户端（并进日志）
- **维度**：密钥与泄露
- **位置**：`crates/xiaoguai-llm/src/gemini.rs:328-372`（key 进 URL query）→ `crates/xiaoguai-llm/src/backend.rs:11-12`（`Network(String)` Display）→ `crates/xiaoguai-agent/src/react.rs:221-242`（`e.to_string()` → `AgentEvent::Error`）→ `crates/xiaoguai-api/src/sse.rs:21`（→ SSE `error` 事件给客户端）
- **证据**：Gemini 用 query 参数鉴权（`...:streamGenerateContent?alt=sse&key={api_key}`）。reqwest 0.12 的 `Error` Display 末尾追加 ` for url (...&key=<KEY>)`，**只对 userinfo password 打码，不对 query 打码**。该串经 `LlmError::Network` → `AgentEvent::Error.message` → SSE 原样发回前端 + 写服务端日志。HotL `RedactionRules` 只作用于 tool-call args，不碰错误消息。
- **风险**：任何 Gemini 侧网络错误（DNS/超时/重置/TLS）都会把完整 Gemini key 泄露给前端用户和日志。其他 backend 用 header 鉴权，**不受影响**；仅 Gemini 中招。
- **修复**：改用 `x-goog-api-key` header 而非 URL query；或在构造 `LlmError::Network` 前对 reqwest 错误串做 `redact_str`；并为 query-param 形式的 key 补 redaction 规则（另见 SEC-17）。

### SEC-05 DingTalk 签名不覆盖请求体，且无时间戳新鲜度校验
- **维度**：IM Webhook
- **位置**：`crates/xiaoguai-im-dingtalk/src/lib.rs:161-175`
- **证据**：签名输入只有 `format!("{timestamp}\n{app_secret}")`，请求 body 不在签名内；`timestamp` 解析后从不与当前时间比对。这是钉钉官方算法的固有特性，但叠加放大风险：(1) 任一被观测到的合法 `(timestamp, sign)` 可配**任意 body 重放**；(2) 无过期 → 捕获的签名**永久有效**。对照 Feishu/WeCom/Slack 都把 body 纳入签名。
- **风险**：webhook 直接暴露公网。签名一旦从日志/反代/镜像流量泄露，攻击者可向 agent 注入任意消息体，触发任意 LLM 对话/动作并消耗额度。
- **修复**：`verify` 中加时间戳新鲜度校验（钉钉为毫秒 epoch，建议 ≤5 分钟，参照 Slack 的 `TIMESTAMP_TOLERANCE_SECS`）。这是该协议下唯一能补救「body 不被签名」的手段。

---

## 🟡 MEDIUM

### SEC-06 全局 `CorsLayer::permissive()`，源/方法/头全放行
- **维度**：认证与 API
- **位置**：`crates/xiaoguai-api/src/routes/mod.rs:338`（注释 `:36` 自承收紧待生产 auth，但从未落地）
- **风险**：任意网页可跨站调用本服务状态变更端点。认证走 HTTP Basic / `X-Xiaoguai-Token`（非 Cookie，permissive 不带凭据），凭据窃取风险有限；但在默认无认证模式（SEC-01）下，任意站点可驱动 localhost 上的 agent API。
- **修复**：改显式 allow-list（新增 `server.allowed_origins`），默认仅同源/回环，生产路径不用 `permissive()`。

### SEC-07 自包含路由把原始错误字符串回显给客户端
- **维度**：认证与 API
- **位置**：`crates/xiaoguai-api/src/routes/providers.rs:181-187`、`crates/xiaoguai-api/src/routes/memory.rs:65-70`
- **证据**：这两个自包含 router **不走**集中式 `ApiError`（后者对 5xx 统一泛化为 `"internal error"` 并只在服务端 `tracing`），而是 `Json({"error": e.to_string()})` 直接回 sqlx/RepoError 细节。
- **风险**：DB/后端错误细节（表名、SQL 片段、路径）泄露到响应体，便于侦察。
- **修复**：这些 router 复用 `ApiError` 的泛化 5xx 映射，原始错误仅 `tracing` 记录。

### SEC-08 敏感数据明文落盘，SQLite 库无显式权限收紧
- **维度**：存储 + 密钥（合并）
- **位置**：创建 `crates/xiaoguai-storage/src/db.rs:64-85`（`create_if_missing`，无 `set_permissions`/0600）；明文列：`llm_providers.api_key`（迁移 `0028`）、`messages.content`（`0001`）、`scheduler_webhook_tokens.token`（`0009`）、`mcp_oauth_tokens.access_token`
- **证据**：DB 文件（及 `-wal`/`-shm`）按进程 umask 创建（典型 0644，同机其他本地用户可读）。restore 路径 `crates/xiaoguai-cli/src/commands/backup.rs:641-647` 已显式 chmod `0o600`，说明团队已认知敏感性——但**实时创建路径漏了同样加固**，前后不一致。AES-256-GCM at-rest 原语 `crates/xiaoguai-mcp/src/auth/at_rest.rs` 已实现但**尚未接线**到持久化路径。
- **风险**：DB 备份泄露 / 主机沦陷 = 全部 provider key + IM token + 对话明文外泄；同机多用户场景下默认权限即可读。
- **修复**：`db::connect()` 创建后（Unix）对 DB 及 `-wal`/`-shm` `set_permissions(0o600)`，与 restore 路径一致；把已实现的 at-rest 加密接线到 token/provider key 持久化路径。

### SEC-09 L1 进程沙箱（`xiaoguai-mcp-exec`）不隔离文件系统与网络
- **维度**：执行与沙箱
- **位置**：`crates/xiaoguai-mcp-exec/src/runtime.rs:98-116`（`network/filesystem/subprocess: true`）、`crates/xiaoguai-mcp-exec/src/exec.rs:215-252`
- **证据**：`python3 -I` 只隔离 env/sys.path，不阻止 `open('~/.ssh/id_rsa')`、读 SQLite 库、`urllib.urlopen()` 外发、`os.system()`。#243 的处理是「把能力说清楚」（`tools.rs:56` 诚实标注），而非真隔离。真隔离在 L3 wasm（`wasmtime_python.rs:218` 无 env/无 preopen/`network:false`），但需 operator 显式注册。
- **风险**：一旦 operator 注册 L1 而非 L3，`execute_python` 成为「读任意文件 + 任意外发」通道，唯一拦它的就是默认放行的 HotL gate（SEC-02）。env 类密钥被 `ENV_ALLOWLIST` 挡住（有测试钉住），但落盘的密钥/DB 挡不住。
- **修复**：默认/强制走 L3 wasm；若必须提供 L1，注册时要求声明 netns/容器 egress 隔离，否则拒绝启用。

### SEC-10 L1 资源限制不完整且可能静默失效
- **维度**：执行与沙箱
- **位置**：`crates/xiaoguai-mcp-exec/src/exec.rs:230-231`（`ulimit -v {mem_kb} 2>/dev/null; exec ...`）
- **风险**：(1) `2>/dev/null` + `;`（非 `&&`）→ `ulimit -v` 失败时（macOS 普遍不支持）python 仍以无内存帽运行；(2) 无 `ulimit -u` → fork bomb 可在超时窗口内耗尽 PID/内存；(3) 无 `ulimit -f` → 写满磁盘。超时 + 进程组 kill 只兜底墙钟，挡不住窗口期资源耗尽。prompt-injection 可触发的 DoS。
- **修复**：用 `&&` 串联并检测 `ulimit` 返回值，失败则拒绝执行；补 `ulimit -u`/`ulimit -f`；Linux 优先 `prlimit --as` 做硬限。

### SEC-11 自愈审批点 / HotL 决策端点的 owner 鉴权是「可选」
- **维度**：执行与沙箱
- **位置**：`crates/xiaoguai-api/src/routes/mod.rs:279`（`if let Some(validator)` 才挂 `require_auth`），受影响 `:244` `POST /v1/incidents/{id}/approve-repair`、`POST /v1/hotl/decisions`；`approve_repair` 实现 `crates/xiaoguai-api/src/incident_pipeline.rs:444-484`
- **证据**：`approve_repair` 把「`awaiting_approval→repairing` 状态迁移」当唯一审批守卫，随后用**完整 toolbox + 同一默认放行 gate** 跑 Executor turn。代码只对 `/v1/mcp/serve` 在无鉴权时打响亮警告，却没对 approve-repair / hotl decisions 这类人审决策点做同等提示。
- **风险**：未配 owner 鉴权时（SEC-01 默认态），审批/决策端点对同网段任意来访者开放——「human-on-the-loop」的 human 可被冒充，self-healing 人工批准形同虚设。（已确认无自动 analyze→approve 链，故非自动放行，但鉴权可选叠加默认放行 gate 时审批语义整体塌陷。）
- **修复**：审批/决策类端点强制要求 owner 鉴权（无配置时直接 503 或启动期硬告警），与 mcp serve 同等对待。

### SEC-12 仅 Slack 有重放防护，Feishu/WeCom/DingTalk/Discord 均无时间戳新鲜度
- **维度**：IM Webhook
- **位置**：Feishu `crates/xiaoguai-im-feishu/src/lib.rs:117-141`、WeCom `crates/xiaoguai-im-wecom/src/lib.rs:180-196`、Discord `crates/xiaoguai-im-discord/src/signature.rs:8-11`（文档自承 stateless，调用方 `lib.rs:163-181` 未补检查）；正例 Slack `crates/xiaoguai-im-slack/src/signature.rs:43-45`
- **风险**：对这四家，一条合法已签名请求可**原样重放**，网关每次返回 200 并 `spawn_agent_reply`，导致重复 agent 运行（重复回复、重复 token 花费、重复触发工具动作）。公网且 IM 路由不经 owner-auth。
- **修复**：各 `verify` 统一加时间戳容差窗口（复用 Slack 模式）；Discord 在 `parse()` 内解析 `x-signature-timestamp` 并校验。

### SEC-13 入站事件无去重，`event_id` 被解析但从未消费
- **维度**：IM Webhook
- **位置**：字段定义 `crates/xiaoguai-im-gateway/src/provider.rs:40`（注释写明 "used for de-duplication"），消费侧 `crates/xiaoguai-im-gateway/src/router.rs:165-171` 完全未引用
- **风险**：各 IM 平台收到非 2xx/超时会重投（Slack 靠 `X-Slack-Retry-Num` 自行丢弃，但属 Slack 专属，gateway 层无统一去重）。与 SEC-12 叠加：平台正常重投 + 攻击者重放都触发重复 agent 执行。注释承诺了去重但实现缺失。
- **修复**：gateway 入站基于 `(provider, event_id)` 做幂等去重（SQLite 短 TTL 表即可，符合 DEC-033），命中已处理则直接 200 丢弃。

### SEC-14 WeCom corp_secret 进 URL query → transport 错误 → 服务端日志
- **维度**：IM + 密钥（合并）
- **位置**：`crates/xiaoguai-im-wecom/src/api.rs:110-119`（`gettoken?...&corpsecret={}`），失败路径日志 `crates/xiaoguai-im-gateway/src/router.rs:287`（`?err`）
- **证据**：`.send()` 失败时 `ProviderError::Transport(format!("auth send: {e}"))` 携带含 `corpsecret` 的 URL，在 reply 失败路径以 `?err` 落日志。对照 DingTalk/Feishu 把 secret 放 JSON body，reqwest 错误不含 body，**不泄露**。
- **风险**：WeCom 企业 secret 在网络抖动时进入服务端日志（持久，可能被采集外送）。
- **修复**：WeCom token 获取改用 POST body，或拼 transport 错误前对 URL 做 redaction。（注：当前出站客户端正常路径未打印含 secret 的 URL，仅错误路径泄露。）

### SEC-15 CLI 审计写入回退到编译进二进制的 dev HMAC key，审计链可伪造
- **维度**：密钥与泄露
- **位置**：`crates/xiaoguai-cli/src/commands/code.rs:33-36`、`crates/xiaoguai-cli/src/main.rs:1523-1526`；默认值 `crates/xiaoguai-config/src/lib.rs:470`（`hmac_key: "dev-only-change-me-32-bytes-min"`）
- **证据**：`xiaoguai code` / `xiaoguai schedule` 的审计追加在 `XIAOGUAI_AUDIT_SIGNING_KEY` 未设时**静默回退到众所周知的硬编码 dev key**。`serve` 路径正确拒绝（无 key → 503，`core/src/lib.rs:267-304`），CLI 路径不一致。
- **风险**：用已知 key 签名的审计行可被任何人伪造/篡改且仍通过 `verify_chain`，破坏防篡改保证。
- **修复**：CLI 与 server 一致——env 缺失时拒绝写审计或显式警告，不 fallback 到 dev key。

### SEC-16 Owner 密码以明文存于 sessionStorage
- **维度**：前端
- **位置**：`frontend/chat-ui/src/client.ts:12-44`、`frontend/admin-ui/src/client.ts:12-44`（两份相同），消费 `frontend/shared/src/index.ts:1411-1417`
- **证据**：`sessionStorage.setItem('xiaoguai.basic.password', password)`，每次请求拼成 `Authorization: Basic`。
- **风险**：DEC-033 下这是系统**唯一**凭据。sessionStorage 对页面 JS 完全可读，一旦出现任何 XSS（含未来依赖回归），攻击者拿到的是**可离线复用的明文密码**，且 chat-ui / admin-ui 同源共享。比 HttpOnly cookie 可窃取性最高。
- **修复**：优先改后端签发 HttpOnly+SameSite cookie 会话；若保留 Basic，密码只存内存（刷新后重登），并文档明确「必须 HTTPS / 仅 localhost」。

---

## 🟢 LOW

### SEC-17 audit / trace redaction 模式覆盖不全
- **维度**：密钥与泄露 — `crates/xiaoguai-types/src/redact.rs:19-39`（被 audit + observability 共用）
- 仅覆盖 email、IPv4、`Bearer <token>`、`AKIA…`。**漏** `sk-…`（OpenAI/DeepSeek）、`ghp_…`/`github_pat_…`、`xoxb-…`（Slack）、`AIza…`（Google）、`key=…` query、通用高熵串。用户在 prompt/工具参数粘贴的 key 会未脱敏进 audit_log 和 OTLP trace——而这正是防泄露的兜底层。
- **修复**：补 `sk-[A-Za-z0-9]{20,}`、`xox[baprs]-…`、`gh[pousr]_…`、`AIza…`、`key=` 等模式 + 高熵兜底。

### SEC-18 无任何速率限制
- **维度**：认证 — 全仓 0 命中 rate-limit（`auth.rs:80-103`、`scheduler_public.rs:31-63`、`routes/incidents.rs:147-181` 均无失败计数/节流）
- 单 owner 下主要风险是对口令 / webhook token 的在线暴力破解无节流。token 有 122 bit 熵（UUIDv4），口令强度由 owner 自定，弱口令时风险上升。
- **修复**：对认证失败路径加 IP/全局节流，或连续失败指数退避。

### SEC-19 webhook token 明文存储且 list 端点每次回显完整 token
- **维度**：密钥 + 前端（合并）— `crates/xiaoguai-api/src/routes/admin.rs:329-392`、前端 `frontend/shared/src/index.ts:352`/`:1647-1677`、`frontend/admin-ui/src/panes/Scheduler.tsx:279-292`
- 创建注释称 "returned exactly once"，但 `GET /v1/admin/scheduler/tokens` 每次列出明文 token；DELETE 把 token 放 URL 路径（进访问日志）。受 owner-auth gate 保护故 LOW。
- **修复**：存 token 哈希，list 只返回前缀 + 内部 id，revoke 按 id 操作；明文仅创建时返回一次。

### SEC-20 coding workspace 可写 `.git/hooks/*` → `git commit` 触发任意代码执行
- **维度**：执行 — `crates/xiaoguai-coding/src/tools.rs:46-103`（`abs()` 把 `.git/hooks/pre-commit` 当 root 下普通路径放行）、`:195-199`（`git_commit` 走系统 git 会执行 hook）
- 模型可 `edit_file` 写 hook 再 `git_commit` 触发——一条不像「执行代码」的代码执行路径。两步都受同一 HotL gate，叠加 SEC-02 默认放行时可达。
- **修复**：git 子进程统一加 `-c core.hooksPath=/dev/null` 禁用工作区 hook。

### SEC-21 Telegram secret 为 None 时完全跳过验签
- **维度**：IM — `crates/xiaoguai-im-telegram/src/webhook.rs:44-56`（`let Some(expected) = secret_token else { return Ok(()) }`）
- 当前生产未 wiring Telegram（仅 feishu/dingtalk/wecom 挂载），暂未暴露；但一旦接入且未配 `secret_token`，webhook 接受任意未认证 POST（Telegram secret-token 是其唯一鉴权）。
- **修复**：接入时强制 `secret_token` 非空（仿 core 中 feishu/wecom 的 `build_*_gateway`：env 空则不挂载）。

### SEC-22 `git_push` 的 remote/branch 为位置参数，存在 git 选项注入面
- **维度**：存储/注入 — `crates/xiaoguai-coding/src/tools.rs:210-212`（`git::run(root, &["push", remote, branch], None)`）
- remote/branch 来自 LLM 工具调用，无 `--` 分隔；`-` 开头的 remote（如 `--receive-pack=...`）理论上可被 git 当选项 → 命令执行面。缓解：受 HotL（`tool_call.git_push`）门控 + 审计，实际可达性低。
- **修复**：remote/branch 前插 `--` 分隔符，或拒绝 `-` 开头的值（fail-fast）。

### SEC-23 eval `run_suite` 接受任意 `cases_dir` 且 `suite_name` 未校验 `..`
- **维度**：存储/注入 — `crates/xiaoguai-api/src/eval.rs:236-247`，路由 `crates/xiaoguai-api/src/routes/admin.rs:170-180`（`POST /v1/admin/eval/run`）
- `suite_name` 仅校验非空、未过滤 `..`；`cases_dir` 为完全任意路径。属 admin 路由、单 owner 本就有文件系统权限，危害低。
- **修复**：对 `suite_name` 拒绝 `..`/绝对路径（保持「只能落在 suites_dir 内」不变式）。

### SEC-24 RAG 引用链接 `source_uri` 直接进 `<a href>`，无协议白名单
- **维度**：前端 — `frontend/chat-ui/src/citations.tsx:44, 58-66, 78-85`
- citation 来自后端 RAG 块；语料投毒/后端绕过时 `javascript:`/`data:`/`obsidian:` 可原样落 href。`target="_blank"` + `rel=noopener` 缓解了多数 JS 协议，但自定义协议可触发本机应用跳转。纵深防御缺口。
- **修复**：`anchoredHref` 加协议白名单（`http/https/file/obsidian/r2r`），不匹配则渲染纯文本。

### SEC-25 后端可配置链接 href 无协议校验
- **维度**：前端 — `frontend/chat-ui/src/AiDisclosureBanner.tsx:87`（`config.link_to_disclosure`）、`frontend/admin-ui/src/panes/Marketplace.tsx:107`（`entry.source_url`）
- 单 owner 下配置者=受害者，风险低；均已带 `rel="noopener noreferrer"`。
- **修复**：与 SEC-24 共用 `safeHref()` 白名单工具函数。

### SEC-26 两个 SPA 均无 CSP
- **维度**：前端 — `frontend/chat-ui/index.html`、`frontend/admin-ui/index.html`（无 CSP meta；响应头属后端范围，本维度未见）
- 当前渲染层干净，但应用核心功能就是渲染 LLM 不可信输出，CSP 是唯一兜底层。叠加 SEC-16（凭据在 sessionStorage），一旦 XSS 无任何缓解。
- **修复**：后端为两个 UI 下发 `Content-Security-Policy`（至少 `default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'`；当前无内联脚本，落地成本低）。

---

## 已验证安全（重点正面结论，避免误改）

- **口令/签名常数时间比较**：HTTP Basic（`auth.rs:108-116` `ct_eq`）和 7 个 IM 适配器验签均常数时间，无内容短路。✅
- **SQL 全量参数化**：所有 `format!` 进 SQL 的只插值编译期 `&'static str` 列名常量，变量值一律 `.bind(?)`。覆盖 storage 全量 + incident/teams/memory store。无注入。✅
- **路径遍历（coding 工具）**：`tools.rs:abs()` 词法校验 + canonicalize 防 symlink 逃逸，有测试钉住。✅
- **zip-slip / tar**：restore 有 `archive_path_is_safe`；RAG loader 只读入内存不落盘；self_update 按文件名读取。✅
- **L3 wasm 沙箱**：无 env / 无 preopen / `network:false`，有 env 泄漏测试。真隔离。✅
- **mcp-exec / git 子进程 env 清理**：`ENV_ALLOWLIST` + `env_clear` + `GIT_TERMINAL_PROMPT=0`（与 SEC-03 的 MCP supervisor 形成对比，后者漏了）。✅
- **consult/execute 分离（T5）**：写工具在 consult 模式直接 Deny，fail-closed。✅
- **provider key 不进 API 响应**：`ProviderView` 投影为 `has_api_key: bool`，有单测断言。前端只消费布尔。✅
- **前端渲染层**：`react-markdown@10` 无 `rehype-raw`，raw HTML 转义，`javascript:` 被默认 URL transform 剥离；三个包源码零 `dangerouslySetInnerHTML`/`innerHTML`/`eval`；SSE 经 `JSON.parse`→React state 不插 DOM；外链全带 `rel=noopener noreferrer`；依赖 lockfile 抽查无已知高危版本。✅
- **仓库无真实硬编码密钥**：所有命中均为测试占位符 / 官方文档示例 / 标注 dev-only 的本地配置。✅
- **IM 验签先于解析**：无未验签即处理 body 的路径；空 secret 时不挂载路由（避免「空密钥放行一切」）。✅

---

## 建议修复顺序

1. **SEC-01 + SEC-02**（CRITICAL，根因同源）：把默认态改 fail-closed —— 默认 `127.0.0.1` + 非回环无 auth 拒启动；高危 scope 内置 fail-closed 默认 HotL 策略。这两条修完，下游一批 MEDIUM（SEC-06/07/11）的实际暴露面大幅收敛。
2. **SEC-03**（HIGH）：`StdioMcpClient::spawn` 加 `env_clear()` —— 一行级修复，挡住审计签名密钥泄漏。
3. **SEC-04 + SEC-14 + SEC-17**（key 进 query/日志一类）：Gemini 改 header、WeCom 改 POST body、补全 redaction 模式。
4. **SEC-05 + SEC-12 + SEC-13**（IM 重放）：统一加时间戳新鲜度窗口 + gateway `event_id` 去重表，一并解决。
5. **SEC-08 + SEC-15**（落盘/审计完整性）：DB 文件 0600 + 接线 at-rest 加密；CLI 审计 key 与 server 一致 fail-closed。
6. 其余 LOW 视精力排期。

> 注：若 #243 mcp-exec quarantine 仍未解除，SEC-09/10 的 L1 沙箱风险只在 operator 主动注册 L1 后端时才暴露——可结合 quarantine 决策一并处理。
