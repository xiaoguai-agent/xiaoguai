# Xiaoguai 小怪

> **以 Rust 实现、审计优先、调度原生的本地化 agent 平台。**
>
> *Your Little Agent for Big Work · 小怪不小，能办大事*

[English](README.md) · **简体中文**

**文档：** 手册源文件位于 [`docs/book/`](docs/book/) —— 用 `mdbook build docs/book` 在本地构建（见下文「文档」一节）。

小怪是一个可自托管的 AI agent 平台，面向技术个人、小团队，以及任何有合规或可追溯性约束的人。每一次工具调用都会写入一行 HMAC 链式审计记录。每一个定时任务都带有重试策略、可回放的对话记录，以及一个 reason 字段。每一次模型交互都有回归评测作为安全网。整个系统作为单个自包含的 Rust 二进制文件交付，内嵌 SQLite 存储 —— 没有外部数据库、没有 Python 运行时、没有 JVM、热路径上也没有 JS 服务。

我们并不打算在提示词魔法上赢过那些 prompt 厂商，不打算在 UI 打磨上赢过那些界面厂商，也不打算在托管上赢过那些市场厂商。我们竞争的是工程上的严肃认真 —— *模型并不可靠，但系统可以可靠。*

## 与众不同之处

| 能力 | Xiaoguai | n8n | Dify | OpenWebUI / LobeChat |
|---|---|---|---|---|
| **审计优先的控制台。** `Today` 是默认落地页 —— 每一次 chat / IM / 定时运行都附带 HMAC 链式审计元数据。聊天只是次要入口。 | 一等公民 | 仅工作流运行 | 仅工作流运行 | 聊天优先；审计不外露 |
| **调度原生（被动 → 反应 → 主动）。** Cron + 文件监视器 + webhook + LLM 发起的运行，带按用户的预算和一个必填的 `reason` 字段。 | 一等公民 | 触发器强 / agent 弱 | 仅 Cron | 无 |
| **MCP 双向。** 既消费 stdio / SSE / streamable-HTTP MCP 服务器，*也* 在 `/v1/mcp/serve` 发布自己的工具箱。外部 agent 看到的，正是内部 agent 看到的。 | 一等公民 | 仅消费方 | 仅消费方（v1.6+） | 有限 / 经由插件 |
| **带一等引用的 RAG。** `ContentBlock::Citation` 是一个带类型的变体 —— 源 URI、行范围、预览、得分。无法提供引用的适配器不得静默输出无来源文本。 | 一等公民 | 原生不支持 | UI 中有引用，schema 不透明 | 有引用；长期存在 bug（#12655、#20829） |

## 快速开始

小怪是 **运行在内嵌 SQLite 文件之上的单个二进制文件** —— 不需要 Postgres、不需要 Redis、不需要 Docker。下面每条路径最终都殊途同归：一个 `xiaoguai` 进程在 `:7600` 上提供服务，开箱即用、基于 `MockBackend` 自包含运行。挑一条与你手头条件匹配的即可。

任意一条都可以用 `curl http://localhost:7600/healthz` → `ok` 来验证，或者运行内置的自检 `xiaoguai doctor`；用 `xiaoguai service install` 让它跨重启持续运行。各方法的预期输出与冒烟测试见：[docs/user-guide/install-and-verify.md](docs/user-guide/install-and-verify.md)。

**每种方法能给你什么：**

| 方法 | 需要工具链 / root | 内置 web UI | 最适合 |
|---|---|---|---|
| **A. pip / pipx** | 否 | ✗ —— 仅 API + CLI | 最快上手；脚本化；用 CLI/API 驱动的服务器 |
| **B. .deb / .rpm / tarball** | root（systemd） | ✓ chat 在 `/`，admin 在 `/admin/` | 应当提供浏览器 UI 的受管主机 |
| **C. 从源码** | Rust 工具链 | ✗ —— 仅 API + CLI | 开发 / 自定义构建 |
| **D. Docker** | Docker | ✓ | 一条命令拉起完整栈 |

> **`http://localhost:7600/` 没有网页？** 在 **pip** 和 **从源码** 安装下这是 *预期* 行为 —— 它们只交付 API + CLI，所以 `/` 返回 404，而 `/healthz` 和 `/v1/**` 工作正常。要拿到浏览器控制台，请安装一个软件包或使用 Docker（B / D），或者把 pip 安装指向一份内置 UI —— 见下文「Web UI」一节。无论哪种方式，你现在就能从终端聊天：`xiaoguai chat --prompt 'hello'`。

### 方法 A —— pip（无需工具链、无需 sudo、全平台）—— 推荐

```bash
pip install xiaoguai
xiaoguai serve   # :7600, auto-creates ~/.xiaoguai/data.db, no config needed
```

在 Debian 12 / Ubuntu 24 及其他 PEP 668 “externally-managed”（外部托管）系统上，向系统 Python 执行 `pip install` 会被阻止 —— 改用 **pipx**（它会隔离应用，同时仍把 `xiaoguai` 放到 PATH 上）：

```bash
sudo apt install -y pipx && pipx ensurepath
pipx install xiaoguai      # then reopen the shell, or: exec $SHELL
```

它会安装一个平台 wheel，其中捆绑了 PATH 上的原生 `xiaoguai` 二进制（macOS arm64/x86_64、Linux x86_64/aarch64）—— 无需 root，在 venv 内即可工作。它提供 API + CLI；要获得内置 web UI，请使用原生软件包（方法 B）或 Docker（方法 D）。离线健全性检查，无需服务器：`xiaoguai chat --mock --prompt 'hello'`。

### 方法 B —— 预构建软件包（无需工具链，捆绑 web UI）

安装后 systemd 单元会自动启动；打开 `http://<host>:7600/`（chat）和 `/admin/`（控制台）。

| 平台 | 命令 |
|---|---|
| Debian / Ubuntu (amd64) | 从 [latest release](https://github.com/xiaoguai-agent/xiaoguai/releases/latest) 下载 `xiaoguai-cli_*_amd64.deb`，然后 `sudo apt install ./xiaoguai-cli_*_amd64.deb` |
| RHEL / Fedora / Rocky (amd64) | 从同一 release 下载 `xiaoguai-cli-*.x86_64.rpm`，然后 `sudo rpm -i xiaoguai-cli-*.x86_64.rpm` |
| 裸机 tarball (amd64 / arm64, glibc 2.35+) | 下载 `xiaoguai-vX.Y.Z-<arch>-unknown-linux-gnu.tar.gz`，解压，然后 `sudo bash scripts/install.sh`（systemd） |

### 方法 C —— 从源码（需要 Rust 工具链）

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai
cargo install --path crates/xiaoguai-cli --locked
xiaoguai serve   # boots on embedded SQLite (~/.xiaoguai/data.db), :7600, no config needed
```

这条路径提供 API + CLI；内置的 chat/admin web UI 只随软件包（方法 B）和 Docker 镜像（方法 D）交付。要做一次无需网络、甚至无需启动服务器的健全性检查：

```bash
xiaoguai chat --mock --prompt 'hello'
```

沙箱化的代码执行 MCP 服务器（`xiaoguai-mcp-exec`）从同一工作区构建：`cargo install --path crates/xiaoguai-mcp-exec --locked`。

### 方法 D —— Docker（一条命令，完整栈 + 捆绑 web UI）

```bash
docker compose -f deploy/docker-compose.yml up --build
# first build ~2 min, then open http://localhost:7600/
```

需要 Docker Compose **v2 插件** —— 用 `docker compose version` 检查。如果报错（`unknown shorthand flag: 'f'`），说明插件缺失：安装 `docker-compose-plugin`，或者干脆改用方法 A / B / C。

`xiaoguai serve` 是各处通用的规范入口。遗留的 `xiaoguai-core` 垫片仍然可用（.deb 为 systemd 向后兼容而接入了它）。关于真实 LLM 提供方、MCP 注册、admin 控制台以及配置细节，见 [`docs/user-guide/quickstart.md`](docs/user-guide/quickstart.md)。

### 第一次聊天 —— 与你运行中的服务器对话

开箱即用时，`xiaoguai serve` 基于内置的 `MockBackend` 启动，因此服务器一起来往返就能跑通：

```bash
xiaoguai chat --prompt 'hello'    # talks to the :7600 server you just started
```

`chat` 会自动创建会话、流式返回回复，并经由你已注册的提供方 + HotL + 审计进行路由 —— 无需操心 session id。注册一个真实提供方即可获得真实回答（交互式，会写入本地 DB）：

```bash
xiaoguai init                     # pick a provider, paste its API key, set default
# restart `xiaoguai serve`, then:
xiaoguai chat --prompt 'introduce yourself in three sentences'
```

想要保留历史的多轮对话？用 `xiaoguai repl`。离线工作或没有服务器？保持直连模式 `xiaoguai chat --mock --prompt 'hello'`（或 `--ollama-url http://localhost:11434`）。

### Web UI

一个浏览器控制台 —— **chat 在 `/`，运维 admin 在 `/admin/`** —— 随软件包、tarball 和 Docker 镜像（方法 B–D）捆绑，并自动提供服务。**pip 和从源码安装只交付 API + CLI**，所以 `http://localhost:7600/` 按设计返回 404（这是最常见的“是不是坏了？”问题 —— 它没坏）。

要给 pip / 源码安装添加 web UI，从 release tarball 取出已构建的 UI，并把 `server.static_dir` 指向它：

```bash
# x86_64 shown; use the aarch64 tarball on ARM hosts
curl -sL https://github.com/xiaoguai-agent/xiaoguai/releases/download/v1.17.0/xiaoguai-v1.17.0-x86_64-unknown-linux-gnu.tar.gz | tar xz
# the bundled UI lives under share/xiaoguai/static (contains chat-ui/ + admin-ui/)
export XIAOGUAI_SERVER__STATIC_DIR="$PWD/xiaoguai-v1.17.0-x86_64-unknown-linux-gnu/share/xiaoguai/static"
pkill -f 'xiaoguai serve'; xiaoguai serve   # now http://localhost:7600/ (chat) + /admin/ (console)
```

或者把它持久化到 `~/.xiaoguai/config.yaml` 里，省得每次都重新 export：

```yaml
server:
  static_dir: /absolute/path/to/share/xiaoguai/static
```

当 `static_dir` 未设置时，`serve` 会自动探测 `<binary>/static`、`<binary>/../share/xiaoguai/static` 以及 `/usr/(local/)share/xiaoguai/static` —— 这正是软件包和 Docker 镜像零配置就能“开箱即用”的原因。

### 升级

升级方式要与安装方式相匹配（混用方法会让你的包管理器的记账状态失同步）：

| 安装方式 | 升级命令 |
|---|---|
| pip | `pip install -U xiaoguai`（在同一个 venv 内运行） |
| pipx | `pipx upgrade xiaoguai` |
| .deb | 从 [latest release](https://github.com/xiaoguai-agent/xiaoguai/releases/latest) 下载新的 `.deb`，然后 `sudo apt install ./xiaoguai-cli_*_amd64.deb`（单元会重启） |
| .rpm | `sudo rpm -U xiaoguai-cli-*.x86_64.rpm` |
| tarball / 裸二进制 | `xiaoguai self-update` —— 下载并经 cosign 校验最新 release，原地替换二进制（`--check` 仅预览、不实际应用） |
| 从源码 | `git pull && cargo install --path crates/xiaoguai-cli --locked --force` |
| Docker | `docker compose -f deploy/docker-compose.yml up --build -d` |

源码路径需要 `--force`：`Cargo.toml` 在 `main` 上保持 `0.1.0`（release 版本来自 git tag），所以不加 `--force` 的话 cargo 会以为包已经安装而跳过重新构建。

三个常见的坑：

1. **升级后重启 `serve`。** 运行中的进程会把旧二进制留在内存里 —— 用 `pkill -f 'xiaoguai serve'` 再重新启动，对于打包服务则用 `systemctl restart xiaoguai-core`。
2. **`xiaoguai --version` 显示 `0.1.0`？** 要么这是一个从源码的构建（tag → 版本替换只发生在 release 产物里，所以源码构建始终报告 `0.1.0` —— 改用 git commit 来确认），要么有另一个 `xiaoguai` 在你的 `PATH` 上把升级过的那个遮挡了。用 `which -a xiaoguai` 检查；`cargo install` 遗留下来的一个游离的 `~/.cargo/bin/xiaoguai` 通常就是元凶。
3. **你的数据会被保留。** `~/.xiaoguai/data.db` 会原样复用；schema 迁移会在 `serve` 启动时自动运行，所以会话、提供方和审计历史都会延续下来。

> **行为变更（v1.17.0）：** `xiaoguai chat --prompt '...'` 现在默认与运行中的 `xiaoguai serve` 对话 —— 它会自动创建会话，并使用你已注册的提供方 + HotL + 审计。旧的直连 Ollama/Mock 的一次性模式已移到 `--mock` / `--ollama-url` 之后。如果你曾把 `xiaoguai chat` 脚本化为对接 Ollama，请加上 `--ollama-url http://localhost:11434`（或用 `--mock` 走预置后端）。

## 可观测性（可选）

遥测是按需开启的。用 `observability` 这个 cargo feature 构建以暴露 `/metrics`（Prometheus）+ OTLP trace 导出 —— 默认关闭。要拉起本地的 Prometheus/Grafana/OTel-collector 栈，叠加可选的 compose 文件：

```bash
docker compose -f deploy/docker-compose.yml \
  -f deploy/docker-compose.observability.yml up --build
```

## 架构

三层，约 34 个 Rust crate，一个工作区。底部的基底（substrate）是纯数据 + 审计；中间的领域 crate 实现 agent + MCP + RAG + 调度器 + 评测原语；顶部的边缘（edges）是用户实际接触的协议与二进制。

```
edges      ┌──────────────┬──────────────┬──────────────┬──────────────┐
           │ xiaoguai-api │ xiaoguai-im- │ xiaoguai-cli │ xiaoguai-    │
           │ axum REST +  │ gateway      │ chat / eval  │ core         │
           │ SSE, 15+ /v1 │ + im-feishu  │ provider /   │ production   │
           │ endpoints    │ (+dingtalk / │ mcp / remote │ binary;      │
           │              │  wecom       │              │ wires all    │
           │              │  scaffolds)  │              │ crates       │
           └──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┘
                  │              │              │              │
domain     ┌──────┴──────────────┴──────────────┴──────────────┴───────┐
           │                                                            │
           │  xiaoguai-llm     LlmBackend + Ollama / OpenAI-compat /    │
           │                   Mock + LlmRouter + circuit breakers      │
           │  xiaoguai-mcp     stdio / SSE / streamable-HTTP clients +  │
           │                   McpSupervisor (live reload from DB)      │
           │  xiaoguai-agent   Toolbox + ReactAgent::run_stream +       │
           │                   AgentEvent + sliding-window history      │
           │  xiaoguai-rag     R2R HTTP + in-mem fallback + RagMcp-     │
           │                   Adapter + reindex_path                   │
           │  xiaoguai-        Trigger × RetryPolicy × JobRun +         │
           │   scheduler       FileWatch + Webhook + ProactiveChecker + │
           │                   BudgetLedger + 4 PushSinks + SQLite repos│
           │  xiaoguai-runtime run_to_completion / run_streamed /       │
           │                   run_to_sink — shared agent loop          │
           │  xiaoguai-eval    regression + capability suites +         │
           │                   5 graders + EvalRunner + CLI             │
           └──────┬─────────────────────────────────────────────────────┘
                  │
substrate  ┌──────┴─────────────────────────────────────────────────────┐
           │  xiaoguai-types   domain types + ID newtypes               │
           │  xiaoguai-config  Settings (server / db / cache / auth /   │
           │                   audit / scheduler / im / eval)           │
           │  xiaoguai-storage sqlx + embedded SQLite repos +          │
           │                   in-process cache fallback                │
           │  xiaoguai-audit   ChainedAudit (HMAC) + SQLite sink        │
           │  xiaoguai-auth    HotL argument redaction (single-owner    │
           │                   pivot; no OIDC/Casbin — DEC-033)         │
           └────────────────────────────────────────────────────────────┘
```

关于长篇的 crate 依赖规则，以及在哪里接入一个新桥接（trait 在 `xiaoguai-api` 或 `xiaoguai-scheduler`，impl 在 `xiaoguai-core::scheduler_bridge`），见 [`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §3。

## 状态

v1 自 2026-05-24 起功能完整。在 v0.10.0 之上的最终冲刺中落地了十三个 tag；`cargo test --workspace` 报告 **443 passed / 0 failed / 66 ignored**；clippy 和 fmt 干净。

| Tag | 要点 | 计划文档 |
|---|---|---|
| v0.10.1 | 反应式触发器 —— FileWatch + Webhook + `JobRunner::run_loop` | [plan](docs/plans/2026-05-23-v0.10.1.md) |
| v0.6.5  | `PgAuditSink` 引导 + 审计链校验端点 + IM 租户路由 | [plan](docs/plans/2026-05-23-v0.6.5.md) |
| v0.7.4  | IM 网关默认 PG 历史 + 持久化工具轮次 + 回放上限 | [plan](docs/plans/2026-05-23-v0.7.4.md) |
| v0.9.4.1| `McpSupervisor` 在市场安装时实时拾取 | [plan](docs/plans/2026-05-23-v0.9.4.1.md) |
| v0.10.2 | 主动式触发器 —— `ProactiveChecker` + 预算 + reason | [plan](docs/plans/2026-05-23-v0.10.2.md) |
| v0.10.3 | 推送 sink —— Feishu / Telegram / Email / Inbox | [plan](docs/plans/2026-05-23-v0.10.3.md) |
| v0.8.3  | chat-ui 代码块语法高亮 + 复制按钮 | [plan](docs/plans/2026-05-23-v0.8.3.md) |
| v0.11.0 | `xiaoguai-eval` crate —— 回归 + 能力套件 + graders + CLI | [plan](docs/plans/2026-05-23-v0.11.0.md) |
| v0.11.1 | 审计优先控制台 —— Today 视图 + `/v1/admin/today` 端点 | [plan](docs/plans/2026-05-23-v0.11.1.md) |
| v0.11.2 | 评测面板 —— 运行套件 + 把会话转为用例 | [plan](docs/plans/2026-05-23-v0.11.2.md) |
| v0.12.0 | `xiaoguai-runtime` + PG 调度器 repo + 运维接线 + webhook HTTP 路由 | [plan](docs/plans/2026-05-24-v0.12.0.md) |
| v0.12.1 | 自然语言任务定义 + 按运行的合成会话 | [plan](docs/plans/2026-05-24-v0.12.1.md) |
| v0.12.2 | 文件监视器 RAG 重建索引接线 + Obsidian 目录条目 | [plan](docs/plans/2026-05-24-v0.12.2.md) |

完整的 v0.9 → v0.12 总计划在 [`docs/plans/2026-05-23-roadmap-v0.9-v0.12.md`](docs/plans/2026-05-23-roadmap-v0.9-v0.12.md)。

## 合规

小怪是为那些需要向第三方为其审计轨迹辩护的自托管部署而构建的。

- **等保 2.0 三级自检（`三级`）** —— 控制项映射在 [`docs/compliance/dengbao-2.0-l3/`](docs/compliance/dengbao-2.0-l3/)。覆盖 GB/T 22239-2019 中的强制项；运营方仍需与一家有 MPS 资质的测评机构一起完成正式的等级测评。
- **GDPR DPIA 模板** —— 预填的威胁模型与合法性依据工作表，位于 [`docs/compliance/gdpr/dpia-template.md`](docs/compliance/gdpr/dpia-template.md)。

平台在代码中（而不仅是文档里）强制执行的硬保证：

- 为每一次工具调用、定时运行和 IM 路由消息生成 HMAC 链式的 `audit_log` 行。链校验暴露在 `/v1/admin/audit/verify`。
- 单 owner 访问门禁：一个可选配置的用户名/密码（HTTP Basic）在 API 暴露于某个 URL 时对其加以保护（DEC-033 —— 无 OIDC/RBAC/多租户；每个人运行自己的实例）。
- 按用户的主动推送预算，带一个必填的 `reason` 字段 —— 若 reason 为空，sink 可拒绝投递。

## 路线图

**v1.0 —— 已交付。** 上表中的一切，加上完整的 v0.1 → v0.10.0 历史。仓库已为首批用户准备就绪。

**v1.1 —— 尚未排期。** 诚实的计划是 *“等首批用户反馈，然后再定优先级。”* 候选 backlog，依据 [`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §5：

- 用于 `/v1/admin/scheduler/webhooks/...` 的作用域 API token（今天是单 owner 的 HTTP Basic 凭证守住整个 admin 表面）。
- `CompositeExecutor`，让调度器运维可以按 payload 类型分派，而不是当前硬编码的 `RuntimeJobExecutor`。
- Admin-ui 的调度器面板（后端已交付，UI 还没有）。
- `RagClient` 的二进制文件重建索引路径（今天只支持纯文本）。
- 为文件监视来源引入 `notify-debouncer-full`。
- 第一方的可写 Obsidian 连接器（社区服务器是只读的）。
- 在 chat-ui 和 admin-ui 上做浏览器走查截图 + 按面板的视觉 QA —— 从 v0.8.1 起每一个影响 UI 的 tag 都是靠阅读、而非肉眼盯着调出来的。
- 对话分叉、公有云 LLM 提供方配置、`/usage` 端点、多 agent 编排 —— 见路线图 §3 的 v1.0+ 部分。

## 许可证

依据 [Apache License 2.0](LICENSE) 授权。

小怪是开源的：可以自由使用、自托管、修改、嵌入和再分发 —— 包括用于商业和生产用途 —— 遵循 Apache License 2.0 的宽松条款，该许可证在通常的署名要求之上增加了一项明确的专利授权。

完整文本见 [`LICENSE`](LICENSE)；署名见 [`NOTICE`](NOTICE)。

## 文档

完整的手册源文件位于 [`docs/book/`](docs/book/)。在本地构建：

```bash
# Install mdbook and mdbook-mermaid first
cargo install mdbook mdbook-mermaid
# Then:
bash docs/book/test-build.sh
```

---

## Wave-3 特性（v1.2.x / v1.3.x）

Wave 3 在 2026 年 5 月下旬把 33 个特性分支合并进了 `main`。工作区现在通过 **1,191 tests / 0 failed / 92 ignored**。三个 Postgres 桥接在生产中仍接线为返回 `503`，直到 v1.3 落地 —— 见下面的诚实状态部分。

### 已交付的内容

| 特性 | 一句话说明 |
|---|---|
| **Human-on-the-Loop 策略（HotL）** | 按风险分级的审批门禁；每一个 `risk ≥ threshold` 的 agent 动作都会暂停，等待人类 `APPROVE` / `REJECT` 后再继续。 |
| **结果遥测与归因** | 每一个 agent 动作都以 `session_id + tool + latency + cost + outcome` 记录；链读取器在 `/v1/outcomes/chain/{session_id}` 向审计消费方暴露。 |
| **Skill packs（技能包）** | 声明式安装：`POST /v1/skills/install {"slug":"incident-triage"}` 记录技能包行；仓库内交付 7 个包（`ar-collections`、`incident-triage`、`pr-review`、`hr-onboarding`、`rag-legal`、`rag-finance`、`rag-hr`），以 `catalog/skill_packs.json` 作为权威清单。 |
| **主动监视器（`xiaoguai-watch`）** | 新 crate；SQL 轮询与 HTTP 轮询的唤醒，喂给调度器，从而无需专门的 worker 进程就能实现反应式的“每 N 秒检查一次、条件变化时触发”循环。 |
| **异常检测（`xiaoguai-anomaly`）** | 对任意数值时间序列的 Z-score 与 EWMA 检测器；作为一个独立 crate 交付，可被调度器任务和 HotL 策略规则消费。 |
| **新 IM 适配器** | Discord（Ed25519 签名验证）、Telegram（Bot API 长轮询）、Mattermost（WebSocket）、Slack（HMAC 签名验证）—— 四个新的 `xiaoguai-im-*` crate，与既有的 Feishu / DingTalk / WeCom 适配器并列。 |
| **Cloud LLM v2** | `ProviderKind` 新增 `Bedrock`（SigV4）、`AzureOpenAi`、`Mistral` 和 `Groq` —— 全部置于既有的 `LlmBackend` trait 之后；熔断器与成本配额防御自动延续。 |
| **可观测性** | 新 `xiaoguai-observability` crate；按需开启的 Prometheus 抓取端点（`/metrics`）和 OTLP trace 导出；默认零遥测（保留 ADR-0013）。 |

### 快速开始 —— 带遥测

基础的 `docker-compose.yml` 会拉起单个 `xiaoguai-core` 服务（内嵌 SQLite）。在其上叠加 `deploy/docker-compose.observability.yml` 即可获得可选的 Prometheus / Grafana / OTel-collector 栈：

```yaml
# deploy/docker-compose.wave3.yml  (create or adapt from the snippet below)
services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.101.0
    command: ["--config=/etc/otel.yaml"]
    volumes: ["./observability/otel.yaml:/etc/otel.yaml:ro"]

  prometheus:
    image: prom/prometheus:v2.52.0
    volumes: ["./observability/prometheus.yml:/etc/prometheus/prometheus.yml:ro"]
    ports: ["9090:9090"]

  grafana:
    image: grafana/grafana:10.4.2
    environment: {GF_SECURITY_ADMIN_PASSWORD: xiaoguai}
    volumes:
      - "./observability/grafana/provisioning:/etc/grafana/provisioning:ro"
      - "./observability/grafana/dashboards:/var/lib/grafana/dashboards:ro"
    ports: ["3000:3000"]
```

```bash
# Bring up everything
docker compose -f deploy/docker-compose.yml \
               -f deploy/docker-compose.wave3.yml up --build

# Apply wave-3 migrations (run once, idempotent after)
docker compose exec xiaoguai-core xiaoguai migrate run
# Migrations that land new in wave 3:
#   0011_hotl_policies.sql
#   0012_outcomes.sql
#   0015_skill_packs.sql

# Grafana → http://localhost:3000  (admin / xiaoguai)
# Prometheus → http://localhost:9090
```

> 如果你配置了 HTTP Basic 门禁（`auth.username` / `auth.password`），请在 admin 的 curl 中加上 `-u "$USER:$PASS"`。没有 bearer token。

二进制名是 `xiaoguai` —— 不是 `xg`。Wave-3 的 CLI 子命令（`xiaoguai skills …`、`xiaoguai outcomes …`、`xiaoguai hotl …`）都已接线；admin-ui 与 REST API 覆盖相同的表面。

### 文档索引

#### 运维指南（mdbook）

| 章节 | 路径 |
|---|---|
| 主动唤醒 / 监视器 | `docs/book/src/operator/` —— day2.md §"Reactive watcher" |
| HotL 策略 | 待补 —— 见 `docs/plans/2026-05-24-v1.1.3.md` |
| 结果遥测 | 待补 —— 见 `docs/plans/2026-05-24-v1.1.4.md` |
| Skill packs | 待补 —— 见 `docs/book/src/skills/overview.md` |

在本地构建手册：

```bash
cargo install mdbook mdbook-mermaid
bash docs/book/test-build.sh
```

#### Runbook

| Runbook | 文件 |
|---|---|
| 可观测性（Prometheus + OTLP） | [`docs/runbooks/observability.md`](docs/runbooks/observability.md) |
| 运维 day-2 | [`docs/runbooks/operator.md`](docs/runbooks/operator.md) |
| systemd 加固 | [`docs/runbooks/systemd-hardening.md`](docs/runbooks/systemd-hardening.md) |
| 灾难恢复 | [`docs/runbooks/disaster-recovery-wave3.md`](docs/runbooks/disaster-recovery-wave3.md) |
| Release 签名 | [`docs/runbooks/release-signing.md`](docs/runbooks/release-signing.md) |

#### 架构

| 文档 | 路径 |
|---|---|
| ADR-0013 默认零遥测 | [`docs/architecture/adr/0013-zero-default-telemetry.md`](docs/architecture/adr/0013-zero-default-telemetry.md) |
| ADR-0014 多模态 MCP 架构 | [`docs/architecture/adr/0014-multimodal-mcp-architecture.md`](docs/architecture/adr/0014-multimodal-mcp-architecture.md) |
| ADR-0009 成本配额 + token 炸弹防御 | [`docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md`](docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md) |
| ADR-0008 工具结果溯源 | [`docs/architecture/adr/0008-tool-result-provenance.md`](docs/architecture/adr/0008-tool-result-provenance.md) |
| 多 agent 对等拓扑 | [`docs/architecture/multi-agent-peer.md`](docs/architecture/multi-agent-peer.md) |
| 系统设计（v0.1 起源） | [`docs/architecture/2026-05-21-design.md`](docs/architecture/2026-05-21-design.md) |

#### 合规

现有映射覆盖等保 2.0 L3 和 GDPR（见上面的合规部分）。SOC 2、HIPAA、PCI-DSS、ISO 27001 以及 EU AI Act 的控制项映射在路线图上 —— 尚未编写。

#### API

REST API 表面（15+ 端点）在 [`docs/book/src/api/rest.md`](docs/book/src/api/rest.md) 中描述，MCP 工具箱在 [`docs/book/src/api/mcp.md`](docs/book/src/api/mcp.md)。OpenAPI 规范和 Bruno collection 计划在 v1.3；路由全部在 `crates/xiaoguai-api/src/routes/` 中带类型定义。

#### Skill packs

| 资源 | 路径 |
|---|---|
| 技能包目录（机器可读） | [`catalog/skill_packs.json`](catalog/skill_packs.json) |
| AR Collections | [`packs/ar-collections/README.md`](packs/ar-collections/README.md) |
| Incident Triage | `packs/incident-triage/` |
| PR Review | `packs/pr-review/` |
| HR Onboarding | `packs/hr-onboarding/` |
| RAG —— Legal | `packs/rag-legal/` |
| RAG —— Finance | `packs/rag-finance/` |
| RAG —— HR | `packs/rag-hr/` |

#### 配方与示例

| 配方 | 路径 |
|---|---|
| 多 agent 对等配对 | [`examples/multi-agent/peer-pair/README.md`](examples/multi-agent/peer-pair/README.md) |
| Grafana 仪表盘包 | [`observability/grafana/README.md`](observability/grafana/README.md) |

#### SDK

| SDK | 状态 |
|---|---|
| Python（`xiaoguai` PyPI 包） | 已交付 —— 通过子进程封装二进制；见 `python/xiaoguai/` |
| TypeScript | 计划中（v1.3） |
| Go | 计划中（v1.4） |
| Java | 评估中 |

### 诚实状态 —— 哪些还没到生产就绪

HotL、outcomes 和 skill-pack 这几个表面现在都由真实的 SQLite 支持的存储托底（单用户转向把它们接线了；它们不再返回 `503`）。剩余的缺口：

- **技能包运行时加载器** 还没接线：通过 API 安装一个包会在 `skill_packs` 表中记录该行，但还不会在运行时激活该包的提示词叠加或工具注册。
- 审计端点（`/v1/admin/audit*`、`/v1/audit/exports`）在配置审计 HMAC 签名密钥（`audit.hmac_key`）之前返回 `503`。
- 气隙（air-gapped）的记忆/回忆还在等待一个 Ollama 支持的 embedder（今天唯一真实的 embedder 是 OpenAI 支持的）。

其余的一切 —— 可观测性、IM 适配器、云 LLM 提供方、异常 / 监视器 crate —— 都已完整接线并经过测试。

---

*建于上海 · 2026。*
