# 专家助手启用 — 前置条件与就绪门禁 (v1.34)

「专家助手」是一批策划好的 persona(角色 + 安全准则)。**每个专家在被选用前,必须先满足
它的前置条件**:装好必需的 skill / 配好必需的 MCP server。未就绪的专家在「助手」面板里
**显示但不可选**(灰锁 + 「需先安装 …」提示 + 「去安装 →」跳到技能页)。

这条规则由一个静态目录 + 实时就绪计算实现,对外是 `GET /v1/experts`。

## 目录

- **前置目录**:`crates/xiaoguai-api/catalog/expert_prerequisites.json`。每个专家声明:
  - `persona_name` —— 它 gate 的 persona 显示名(必须与 `xiaoguai-core::persona_seed`
    里 seed 出来的名字一致;`persona_seed` 的 `seed_names_are_stable` 测试钉住)。
  - `required` —— 若干**必需组**。每组是一个 **OR 集**(`any_of`),组内任一项满足即该组满足;
    **所有必需组都满足**,专家才 `ready`。
  - `optional` —— 选装的 marketplace slug,专家启用后客户再按需加装。
- **两类前置项**:
  - `mcp`:一个 marketplace slug —— 装好(`mcp_servers` 有该行)即满足。
  - `package`:主机上的库/CLI —— `command -v <probe>` 能解析即满足(尽力而为)。

## 已内置的三个专家

| 专家(persona 名) | 必需 | 选装 |
|---|---|---|
| **VMware 运维助手** | ① `vmware-policy`(包,`vmware-audit`)② `vmware-monitor` **或** `vmware-aiops` | storage / vks / nsx / nsx-security / aria / avi / harden / debug / log-insight |
| **VMware 网络运维助手** | ① `vmware-policy` ② `vmware-nsx` **或** `vmware-nsx-security` | avi / aria / monitor |
| **数据分析助手** | `postgres` **或** `sqlite` | filesystem / fetch / memory |

> `vmware-policy` 是 VMware 家族的**策略/审计/输入清洗地基库**(其它包都 import 它),
> 装法 `uv tool install vmware-policy`,提供 `vmware-audit` CLI —— 就绪门禁用它探活。

三个 persona 在 serve 启动时 **create-if-never-existed** 自动 seed(owner 归档删除后不会复活)。

## 离线包(本地共享盘 / 内网索引)

这些包都是**离线包**,发布在你自己的本地共享盘或内网索引上,不走公网。安装前:

```bash
# 方式一:内网 PyPI 索引
export UV_INDEX_URL=http://<你的内网索引>/simple      # 或 PIP_INDEX_URL
uv tool install vmware-policy
uv tool install vmware-monitor

# 方式二:本地 wheel 目录(共享盘挂载点)
uv tool install --find-links /mnt/share/vmware-wheels vmware-monitor
```

就绪面板里的 `offline_hint` 会把这段提示展示给操作者。具体索引地址写进你的运维手册。

## 启用一个专家(以 VMware 运维为例)

1. 主机侧:`uv tool install vmware-policy` + `uv tool install vmware-monitor`(或 `vmware-aiops`),
   并配好各自的 `~/.vmware-<name>/{config.yaml,.env}`(见 [vc-ops-assistant.md](vc-ops-assistant.md))。
2. web:技能页 / 一键启动卡里安装对应 MCP(写入 `mcp_servers` 行)。
3. 回到「助手」面板 —— 「VMware 运维助手」的锁消失,可选用。仍未就绪时,提示会**具体列出**
   还差哪一必需组。

## 诚实边界

- `package` 探活是 `command -v`,只证明**可执行在 PATH**,不深入校验版本/可连通性 —— 够做启用门禁,
  真正能否连 vCenter 仍取决于主机配置(有就是有,没有就是没有)。
- `GET /v1/experts` 就绪失败时,chat-ui **fail-open**:不因为接口抖动就锁死所有 persona。
- 普通 persona(目录里没有对应 blueprint)永不加锁 —— 门禁只作用于策划好的专家。
