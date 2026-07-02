# VC 运维助手 — 设置与使用指南

把「VM 运维助手」做成能**真连 vCenter 运维**的助手:它通过 [github.com/zw008](https://github.com/zw008)
的 VMware MCP 技能家族(stdio）真实调用 vCenter/ESXi/NSX/Aria 等,**只用真实数据、绝不编造**。

## 架构(本分支 `feat/vc-ops-assistant` 已建)

要让一个 MCP 驱动的运维助手在对话里工作,需要两块此前缺失的地基,已补上:

1. **persona 注入对话轮**(`turn.rs`,地基①)——会话绑定的 persona 的 `system_prompt`
   现在会注入每一轮普通对话(此前只有 `/orchestrate` 注入)。这让 VM-ops 助手的
   **角色 + 反幻觉 + 安全准则**真正生效。
2. **MCP 工具注入对话工具箱**(`turn.rs` + `McpSupervisor`,地基②)——已启动的 MCP
   server 的工具现在合并进每轮对话的工具箱,agent 可调用。工具的 `MutationHint` 一并生效:
   **只读工具在咨询模式可用,写工具受 consult/HotL 管控**(与 coding 工具同源治理)。

> 仅支持 **stdio** MCP server(HTTP/SSE 暂不支持)。vmware 家族都是 stdio,契合。

## 模式(为什么这样最好)

- **能力 = vmware MCP server**(Python,`uv tool install`),由 xiaoguai 的 supervisor 拉起。
- **vCenter 连接信息在 vmware server 自己的配置里**(`~/.<pkg>/config.yaml` + `.env`),
  在主机上配一次。xiaoguai 的职责是:拉起 server + 把它的工具接进对话 + 由 persona 治理。
- **分层安全**:先用 `vmware-monitor`(只读、代码级强制安全)演示与排障;需要变更时上
  `vmware-aiops`(运维),它对破坏性操作(关机/删除/重配/快照回滚/克隆/迁移)**强制二次确认、无绕过**。
- **反幻觉**:助手手里只有真实工具;查不到/未接入就明说,绝不编清单、状态或文件。

## 安装与连接(主机侧,一次性)

以只读监控为例(其余家族成员同理:`vmware-aiops` / `vmware-storage` / `vmware-vks` /
`vmware-nsx` / `vmware-nsx-security` / `vmware-aria` / `vmware-avi` / `vmware-harden`):

```bash
# 1) 安装 vmware skill(把可执行放到 PATH)
uv tool install vmware-monitor        # 或 pip install vmware-monitor

# 2) 配置 vCenter 连接(该 server 自己的配置,不是 xiaoguai 的)
mkdir -p ~/.vmware-monitor
cp <repo>/config.example.yaml ~/.vmware-monitor/config.yaml   # 填 vCenter 主机/端口/target 名
$EDITOR ~/.vmware-monitor/.env                                 # 填 VMWARE_<TARGET>_PASSWORD,chmod 600
chmod 600 ~/.vmware-monitor/.env

# 3) 自检:本机直接跑一下 MCP server(stdio)
vmware-monitor-mcp     # 能起来即可(Ctrl-C 退出)
```

> `config.yaml` 只存主机/端口/target 引用;**密码只在 `.env`**(`chmod 600`)。

## 在 xiaoguai 里接入

1. **PATH**:确保运行 `xiaoguai serve` 的进程能找到 `vmware-monitor-mcp`(`uv tool` 的 bin 目录在 PATH;xiaoguai spawn 时透传 `PATH`+`HOME`,故 `~/.vmware-monitor/` 可解析)。
2. **安装 MCP**:管理后台 → **MCP / 市场** → 找到 `VMware Monitor (read-only)` → 安装。
   这会写一行 `mcp_servers`(`command=vmware-monitor-mcp args=[]`),supervisor 自动拉起;
   其工具随即合并进对话工具箱(地基②)。
3. **建并绑定助手**:目前**没有预置** VM 运维 persona —— 在「助手」里**新建**一个名字含「运维」或
   「vmware」的 persona(这个命名会触发一键启动卡与检测),把「只读优先 + 反幻觉(有就是有)+
   变更需复述确认」写进它的 `system_prompt`(可参照 zw008 `SKILL.md` 的 Safety Rules);对话页「助手」
   Tab 选它,其提示词即注入本轮(地基①)。*(预置一个 turnkey VM 运维 persona = v1.34.0 待办。)*
4. **(可选)配置目录**:侧栏「配置目录」设一个工作目录 → 助手产出的报告会**真实落盘**到那里。

## 使用

- **只读巡检**(monitor,任意模式):「列出 prod-vcenter 上所有 ESXi 主机的健康状态」
  →助手调真实工具→返回真实清单;**未连接则直说『未连接 vCenter』**。
- **运维变更**(需装 `vmware-aiops`,执行模式):「把 vm-101 关机」→助手先**复述对象+影响+征得同意**,
  且 `vmware-aiops` 自身**二次确认**。
- **咨询模式**:写工具被拦截(预览不执行)。要让 `vmware-monitor` 的只读工具在咨询模式下**自动放行**,
  需把它加入只读信任名单 —— 起 serve 时设 `XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS=vmware-monitor`(#286;
  这些工具在 server 里声明了 `readOnlyHint`)。**不设则保守**:连读工具也当作「可能写」被拦,安全但偏严。
- **审计**:所有工具调用进 HMAC 审计链;高风险作用域默认 fail-closed(无策略则升级审批)。

## 严禁幻觉(产品保证)

- 助手**只**用工具返回的真实数据;**有就是有,没有就是没有**。
- 写文件 = 用真实工具写入工作目录的真实文件,**绝不**谎称已写或生成「假文件」。
- 工具不可用 → 明确告知需安装哪个 vmware skill + 连 vCenter,而非编造结果。

## 诚实的限制 / 待办

- **live 端到端运维需要**:(a) 一个**有额度的 LLM key**(驱动 agent 选工具);
  (b) 一个**真实 vCenter** + 已装并配好的 vmware MCP server。这些在你的环境里,本仓库只提供接入。
- 仅 stdio MCP(HTTP/SSE 待实现)。
- persona 的 `tool_allowlist` 强制(把助手限定到 vmware 工具子集)在对话轮里尚未接线 —— 待办;
  当前靠角色提示词 + 可用工具集约束。
- admin 暂不能在 UI 里填 vCenter 凭据(走 server 的 `config.yaml`/`.env`);UI 凭据录入是后续增强。
