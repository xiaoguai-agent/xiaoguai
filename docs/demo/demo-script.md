# Xiaoguai 现场 Demo 脚本（演讲人照稿）

> 给演讲人逐句照着走的稿子。每个场景都有：**一句话价值 → 差异化 → 现场操作（具体命令/点哪）→ 预期看到 → 串场词**。
> 全程离线、单机、单二进制、内嵌 SQLite，默认端口 `:7600`。
>
> 三条主线，按这个动线串：
> ① **治理 / 合规** —— 一条审计链到底 + HotL 管控 + consult/execute 只读拦截
> ② **自动化运维** —— 装 pack 出 agent 团队 + 编排；自带监控报警 + 自愈闭环
> ③ **离线 / 本土** —— 纯离线起动；飞书 / MiniMax 集成
>
> 预计时长：**12–15 分钟**（可裁剪，每个场景标了可选）。

---

## 0. 开场准备（演讲前 5 分钟，私下做完）

目标：让所有 pane 现场都有内容，consult/execute 能稳定演示，server 起好。

```bash
# (a) 选一个干净的演示库，避免污染你日常的 ~/.xiaoguai/data.db
mkdir -p "$HOME/xiaoguai-demo"
export DEMO_DB="$HOME/xiaoguai-demo/data.db"          # 纯文件路径（场景 2.3 直接喂给 sqlite3）
export XIAOGUAI_DATABASE__URL="sqlite://$DEMO_DB?mode=rwc"

# (b) 审计链签名密钥（demo-seed 用它把审计行签进 HMAC 链；serve 必须用同一个）
export XIAOGUAI_AUDIT_SIGNING_KEY="demo-stage-key-请换成你自己的-至少32字节"

# (c) 打开内置 consult/execute 演示工具（默认关闭，仅 demo 开）
export XIAOGUAI_DEMO_TOOLS=1

# (d) 灌入逼真示例数据：审计链 / 会话 / 定时任务 / token 用量基线+spike / 事件+RCA
xiaoguai demo-seed
#   → 打印「灌了什么 + 打开哪些 pane 看什么」。审计链 verify 绿。
#   想重来：xiaoguai demo-seed --reset  然后再 demo-seed（幂等，不会重复堆数据）

# (e) 起 server（务必在同一个 shell —— 继承上面 4 个环境变量）
#     现场如果要从大屏/局域网访问 UI，再加 --host 0.0.0.0（需 owner 鉴权，见主线③）
xiaoguai serve
#   → ✓ xiaoguai running at http://localhost:7600
#     浏览器打开 http://localhost:7600/        （chat-ui）
#                http://localhost:7600/admin/  （admin-ui，治理面板都在这）
```

> **演讲人备忘**
> - `demo-seed` 写的是 **serve 同一个 SQLite 库**，所以只读 pane 一灌就有，不用重启。
> - 审计链是 **append-only**：`--reset` 只清会话/任务/用量/事件，**不删已签名的审计行**（删了会破坏 HMAC 链）。这点本身就是合规卖点，可以现场点出来。
> - 如果用 pip 安装：命令同名，`pip install xiaoguai` 后直接 `xiaoguai demo-seed` / `xiaoguai serve`。
> - 起 server 建议用一个独立终端窗口（前台跑，方便现场指给观众看启动横幅）。

预检清单（私下确认一遍，避免现场翻车）：

```bash
curl -s http://localhost:7600/healthz        # → ok
xiaoguai stats --by day                       # → 能看到 token 用量，末尾一天有 spike
```

---

## 主线 ① 治理 / 合规

### 场景 1.1 ——「一条审计链到底，且不可篡改」

- **一句话价值**：Agent 做的每一件事——登录、调工具、花钱、改代码、人工审批——都按发生顺序签进一条 HMAC 审计链，导出即合规证据。
- **差异化**：别家是「事后扒日志」；我们是**密码学链式、不可篡改、可一键校验**。删任何一行、改任何一个字段，`verify` 立刻报链断裂。
- **现场操作**：
  1. 浏览器开 `http://localhost:7600/admin/`，点左侧 **活动历史 / Activity**。
  2. 指给观众看：条目按时间排开（`auth.login` → `session.create` → `tool.invoke` → `cost.charge` → `code.edit` → `git.commit` → `hotl.escalate` → `hotl.decision` → `data.export` → `audit.verify`），每条都带**绿色校验徽章**。
  3. （可选，技术观众）切到终端，导出一份合规证据包：
     ```bash
     curl -s -X POST http://localhost:7600/v1/audit/exports \
       -H 'content-type: application/json' \
       -d '{"framework":"soc2","from":"2026-01-01T00:00:00Z","to":"2030-01-01T00:00:00Z","format":"json"}' \
       | head -c 400
     ```
     强调：导出**前**会强制校验链，链断就拒绝导出、返回 409——**没有 `--skip-verify` 这种后门**。
- **预期看到**：一排带绿勾的审计条目；导出 JSON 里带链证明（chain proof）。
- **串场词**：
  > 「企业最怕的是『Agent 替我做了事，但我说不清它到底做了什么』。Xiaoguai 把 agent 的每一个动作都签进这条链——而且是密码学意义上的不可篡改。这不是日志，这是**证据**。一会儿你们会看到，连『被拒绝的操作』也会进这条链。」

### 场景 1.2 ——「只读模式：consult vs execute，拦截写操作不靠模型自觉」

- **一句话价值**：把一次对话锁成**只读（consult）**，写操作在**运行时被网关拦截**——不是靠提示词求模型「请别乱写」，是硬拦。
- **差异化**：业界普遍靠 prompt 约束「你是只读助手」，模型一旦被绕过就失控。我们是**两层防御**：① 只读模式下写工具**对模型不可见**；② 即便模型硬编出写工具名，`ConsultGate` 在**派发前**拒绝，并写一条 `consult.denied` 审计。
- **现场操作**（用内置 demo 工具对 `demo_read_note` / `demo_write_note`，效果稳定可复现）：
  1. 回到 chat-ui（`http://localhost:7600/`），新建一个会话。
  2. **先演 execute（默认）**：composer 下方的模式切到 **Execute**，发：
     > 用 demo_write_note 把便签写成「发布检查清单 v1」，然后用 demo_read_note 读回来。
     - 预期：两个工具都成功，便签被写入并读回。
  3. **再演 consult（只读）**：把模式切到 **Consult**（composer 会变成只读视觉提示），发同一句：
     > 用 demo_write_note 把便签改成「偷偷改一行」，再用 demo_read_note 读。
     - 预期：`demo_read_note` 正常返回；`demo_write_note` 被拦截，返回稳定原因 **「consult mode: write tools are disabled」**，便签**没被改**。
  4. 切回 admin **活动历史**，刷新——多出一条 **`consult.denied`**（resource = `tool:demo_write_note`），同样签进了链。
- **预期看到**：execute 下读写都通；consult 下写被拒 + 审计多一条 `consult.denied`。
- **串场词**：
  > 「注意，我没有改任何提示词去『请求』模型别写。是系统层面：只读模式下，写工具**根本不在模型能看到的工具箱里**；就算它幻觉出一个写工具名，网关在真正执行前就拦了，而且把这次拦截也记进审计链。**治理不能靠模型自觉，要靠机制。**」

> **备忘**：这对 `demo_*` 工具是**内置演示工具**，由 `XIAOGUAI_DEMO_TOOLS=1` 门控，默认关闭、生产不暴露。它们只读/写一个进程内内存便签，不碰任何真实资源。真实的 consult/execute 同样作用于编码工具（`edit_file` 等）和所有 MCP 写工具。

### 场景 1.3 ——「HotL：人在环上，花钱/高危动作要审批」（可选，~1 分钟）

- **一句话价值**：超预算或高危的动作会**挂起等人工审批**，不是先斩后奏。
- **现场操作**：admin **活动历史**里指出 seed 数据中的 `hotl.escalate` → `hotl.decision(approve, by owner)` 这对条目——一次升级、一次人工放行，全程留痕。
- **串场词**：
  > 「Human-on-the-Loop：模型自己跑，但触到预算红线或高危操作，它会停下来等你点头。审批本身也是审计链的一环。」

---

## 主线 ② 自动化运维

### 场景 2.1 ——「装一个 skill pack，立刻多出一支 agent 团队」

- **一句话价值**：装一个技能包 = 启动时自动派生出一组各有专长的 agent + 一个可编排的团队，复杂任务直接交给团队并行 + lead 汇总。
- **差异化**：不是一个大模型硬扛所有事；是**多专家分工 + 一个 lead 综合**。包是声明式 YAML，装上即生效。
- **现场操作**：
  1. 看目录里有什么包：
     ```bash
     xiaoguai skills list | head -20
     ```
  2. 装一个**自带可跑**的会话型团队包（不依赖外部数据，现场最稳）：
     ```bash
     xiaoguai skills install --pack release-notes-team
     ```
     这个包有 4 个 agent：changelog-curator / breaking-change-auditor / bilingual-localizer，外加 lead **release-editor**。
  3. 确认团队已激活：
     ```bash
     xiaoguai skills list --installed
     ```
     - 预期：`release-notes-team` 状态 **active**，列出 4 个 agent。
     - chat-ui 顶部也会出现 **「团队已激活」** 提示 / Expert 选择器里能选到这个团队。
- **预期看到**：installed 列表里包是 active 且带 agent 名；UI 出现团队 badge。
- **串场词**：
  > 「我刚装了一个『发布说明团队』。注意——装包不是下载一段提示词，是系统**启动扫描后自动派生出 4 个有分工的 agent 和一个能被编排的团队**。下一步我们就让这个团队干活。」

### 场景 2.2 ——「把复杂任务交给团队编排」

- **一句话价值**：一个复杂目标，多个成员并行产出，lead 综合成一份成品。
- **现场操作**：
  ```bash
  xiaoguai remote orchestrate \
    --user-id usr_owner \
    --goal "把下面这段 git log 整理成可发布的中英双语 release notes：\
- feat: 新增 demo-seed 一键灌数据\
- fix: consult 模式拦截写工具\
- feat(packs): 装包即派生 agent 团队\
- chore: 升级依赖"
  ```
  （现场也可以临时粘一段真实的 `git log --oneline` 进去，更有说服力。）
- **预期看到**：流式输出——先看到各成员（changelog / breaking-change / 本地化）分头产出，最后 **release-editor** 汇总成一份分组清晰、中英双语的发布说明。
- **串场词**：
  > 「changelog-curator 在按 Keep-a-Changelog 分组，breaking-change-auditor 在判断要不要升大版本，bilingual-localizer 在出中文，最后 lead 把它们缝成一份能直接发出去的稿子。**这就是复杂任务的正确接法：分工 + 综合，不是一个模型硬扛。**」

### 场景 2.3 ——「自带监控报警：token 用量异常，z-score 当场检出」

- **一句话价值**：内置时序异常监控（z-score / EWMA），盯住 KPI——比如 LLM token 花销——异常即报，**在账单失控前**抓住。
- **差异化**：监控、报警、自愈是**内建在同一个二进制里**，不需要外挂 Prometheus / 告警系统。
- **现场操作**（seed 已经在 `token_usage` 里灌了一段平稳基线 + 末尾一个明显 spike）：
  1. 先把 spike 指给观众看：
     ```bash
     xiaoguai stats --by day
     ```
     - 预期：最近一天的 token 总量比前面几天**陡增一大截**。
  2. 把 seed 的用量导成 CSV，喂给内置的「token 花销」异常规格回测（`anomaly test` 走 HTTP，**需要 server 在跑**——开场 (e) 已经起好；back-test 把规格里的 z-score 检测器套在这串点上）：
     ```bash
     # 用开场 (a) 定义的 $DEMO_DB（纯文件路径），导出 ts,value 两列。
     # printf 写表头，跨平台（避免 macOS 的 BSD sed 不支持 `1i` 文本）。
     { printf 'ts,value\n'; \
       sqlite3 "$DEMO_DB" \
         "SELECT ts, total_tokens FROM token_usage WHERE request_id='demo-seed' ORDER BY ts" \
         | tr '|' ','; } > /tmp/demo-usage.csv

     xiaoguai anomaly test \
       --file packs/observability-starter/anomalies/daily-token-spend.yaml \
       --data /tmp/demo-usage.csv --ts-col ts --val-col value
     ```
  3. 预期：回测表里**末尾那个点（spike）被标为 anomaly**，score 远超 3σ 阈值，附一句异常说明。例如：
     ```text
     ANOMALY  TS                        VALUE   MEAN    STD     SCORE  DESCRIPTION
     *        2026-...T10:27:22+00:00   8200.0  1478.1  1796.7  3.7    Z-score 3.74 exceeds threshold 3.00 ...
     summary: 1 anomalies in 15 points (detector: zscore)
     ```
     （seed 灌了 14 个 ~1000 的平稳点 + 1 个 ~8200 的 spike，满足该规格的 `min_count: 7`。）
- **预期看到**：stats 里肉眼可见的 spike；anomaly 回测把它判为异常（z-score > 3）。
- **串场词**：
  > 「这段是平稳基线，最后这一下是 spike——可能是一个跑飞的 loop，或者配错了模型。z-score 检测器一眼就把它揪出来。**监控不是另一个要你运维的系统，它就在这一个二进制里。**」

### 场景 2.4 ——「自愈闭环：告警 → 事件 → 根因分析 → 审批 → 修复」（可选，~1.5 分钟）

- **一句话价值**：从一条告警，到 consult 模式的根因分析（RCA），到人工审批，到 execute 模式的修复，闭环留痕。
- **现场操作**：
  1. admin 点 **事件 / Incidents**：看 seed 的那条 `checkout-api 错误率突增至 13.7%`（状态 resolved）。
  2. 展开它的 **RCA**：根因（新版本对支付网关的同步调用，在网关抖动时耗尽线程池）、置信度 0.86、行动项（回滚 / 加熔断 / 补监控）。
  3. 点出动线：**Analyst 在 consult（只读）模式定位根因 → 人工审批 → Executor 在 execute 模式回滚**——正好串回主线①的 consult/execute。
- **预期看到**：一条带完整 RCA 的已解决事件。
- **串场词**：
  > 「注意根因分析是在**只读模式**做的——分析时它不能改任何东西；要真动手修，得先过人工审批，再切到执行模式。**诊断只读、动手要批**，这就是可治理的自愈。」

---

## 主线 ③ 离线 / 本土

### 场景 3.1 ——「纯离线、单机、单二进制」

- **一句话价值**：一个可执行文件 + 一个 SQLite 文件，无 Postgres / Redis / 外部队列 / 云依赖。可在内网、隔离网、产线机房直接跑。
- **差异化**：不是「云服务的本地版」；是**从架构上就为离线/可控环境而生**（DEC-033 硬约束）。
- **现场操作**：
  1. 指出启动横幅：`✓ xiaoguai running at http://localhost:7600`，状态全在 `~/xiaoguai-demo/data.db` 一个文件里。
  2. （可选）现场断网，重发一条聊天/再跑一次 `xiaoguai stats`——照常工作（用本地 provider / 已有数据）。
  3. 要从局域网大屏访问 UI：
     ```bash
     # 注意：绑非本机地址需 owner 鉴权（SEC-01），别裸奔
     XIAOGUAI_AUTH__USERNAME=owner XIAOGUAI_AUTH__PASSWORD=请设强密码 \
       xiaoguai serve --host 0.0.0.0
     # 找本机 LAN IP： hostname -I   然后浏览器开 http://<IP>:7600/
     ```
- **串场词**：
  > 「这里没有数据库服务器、没有缓存、没有消息队列，更没有连出去的云。一个二进制，一个 SQLite 文件。**金融、政企、产线内网——能落地的 AI agent，首先得能在你信任的网络里独立跑起来。**」

### 场景 3.2 ——「本土集成：飞书 + MiniMax」

- **一句话价值**：接国产 IM（飞书 / 钉钉 / 企微）做触达，接国产大模型（MiniMax）做推理——本土栈开箱即用。
- **现场操作**：
  1. **MiniMax（推理）**：admin → **Providers**，展示已注册的 MiniMax provider（端点 `api.minimaxi.com`，模型如 `MiniMax-Text-01` / `MiniMax-M2`）。
     - （可选）现场切默认模型到 MiniMax，发一条聊天，回一句中文。
     > 提示：国内 key 用 `api.minimaxi.com`（不是国际站 `api.minimax.io`），否则 401。
  2. **飞书（触达）**：指出 seed 的定时任务 `每日 dashboard 巡检`——它的推送 sink 是 `feishu:ops-room`。
     - 串到价值：「巡检/告警结果，直接推到飞书群，人在飞书里就能拍板（HotL 审批也能从 IM 走）」。
- **预期看到**：Providers 列表里的 MiniMax；定时任务里 feishu sink。
- **串场词**：
  > 「推理可以用 MiniMax 这样的国产大模型，触达走飞书。从模型到 IM，整条链路都能落在本土栈上——而且前面讲的审计、HotL、consult/execute 这些治理能力，**一条都不少**。」

---

## 收尾（30 秒）

- **串场词**：
  > 「回顾一下：它**离线单机**起得来；做的每件事都进一条**不可篡改的审计链**；只读模式**机制级**拦写、花钱要**人工审批**；装个包就多一支**agent 团队**做复杂任务；**监控告警自愈**全在一个二进制里；模型和 IM 都能用**本土栈**。
  > 一句话——**一个你管得住的、能在你自己网络里独立跑的 AI agent 平台。**」

---

## 附：命令速查 / 出问题时

| 想干嘛 | 命令 |
|---|---|
| 灌演示数据 | `xiaoguai demo-seed` |
| 清演示数据（保留审计链） | `xiaoguai demo-seed --reset` |
| 起服务 | `xiaoguai serve`（局域网：`--host 0.0.0.0` + owner 鉴权） |
| 自检 | `xiaoguai doctor` / `curl localhost:7600/healthz` |
| 看用量+spike | `xiaoguai stats --by day` |
| 列/装技能包 | `xiaoguai skills list` / `xiaoguai skills install --pack release-notes-team` |
| 团队编排 | `xiaoguai remote orchestrate --user-id usr_owner --goal "..."` |
| 异常回测 | `xiaoguai anomaly test --file <spec.yaml> --data <csv> --ts-col ts --val-col value` |

**踩坑提醒**
- `demo-seed` 与 `serve` **必须用同一个 `XIAOGUAI_DATABASE__URL` 和同一个 `XIAOGUAI_AUDIT_SIGNING_KEY`**，否则数据不在一个库、或审计链 verify 不过。
- consult/execute 的写拦截演示**需要 `XIAOGUAI_DEMO_TOOLS=1`** 才有 `demo_write_note` 这对工具。
- `skills install` / `orchestrate` 走 HTTP，**需要 `serve` 正在运行**。
- 审计链 **append-only**：`--reset` 不删审计行（这是合规特性，不是 bug）。
- MiniMax 国内 key 用 `api.minimaxi.com`。
