# Cherry Studio 风格 chat-ui 全套 IA 重构

**Branch:** `feat/cherry-studio-ui` · **Date:** 2026-06-29 · **Task:** #18 (owner 选「全套 IA 重构」)

## 目标

把 `frontend/chat-ui` 从「单侧栏会话列表 + 隐藏模型选择器」重构为 Cherry Studio
桌面客户端的信息架构,**完全复用现有后端**(personas / teams / sessions /
providers),不引入新服务(守 DEC-033)。

## 现状(scout 已确认)

- 布局:`.layout` = `<SessionList>`(280px 侧栏)+ `<main>`(topbar + routes)。
- 会话扁平列表;专家用 `ExpertPicker` 小 chip;模型选择器是藏在 composer 下的
  原生 `<select>`;消息操作只有 hover 的 复制 / 分支。
- 样式:单一 `styles.css`(~2841 行)+ CSS 变量(亮/暗)。i18n:打包 JSON
  (en/zh-CN/ja),严格 `TranslationShape`,`{{var}}` 插值。

## 目标 IA

```
┌──┬──────────────┬───────────────────────────────┐
│导│ [助手|话题] tab │ 顶栏: 助手名 · 模型选择器▾ · watch/运行中 │
│航│ 🔍 搜索         │                                 │
│栏│ ─助手─          │  消息区(气泡 + hover 操作条:        │
│  │ 通用 / personas │   复制·重新生成·编辑·分支·删除)        │
│⚙ │ ─团队─ teams    │                                 │
│主│ (切到话题 tab:)  │ 输入框 + 模式(执行/咨询) + 团队并行   │
│题│ sessions + 新建 │                                 │
└──┴──────────────┴───────────────────────────────┘
```

- **窄图标导航栏(~52px)**:Logo + 对话 / 技能 / 活动历史 图标;底部 设置(→/admin)
  · 主题 · 语言。收拢今天散在侧栏脚部的东西。
- **列表面板(~270px,Tab 切换)**:
  - **助手** = personas + teams(复用 `listPersonas` / `listTeams`)。含置顶「通用」。
    选中即设为当前/新对话的 persona(`attachPersona`),自动切到「话题」。搜索框。
  - **话题** = sessions(`listSessions`)+「+ 新话题」。
- **聊天顶栏**:醒目 助手名 + 模型选择器(从 composer 提上来)+ watch/运行中。
- **每条消息 hover 操作条**:复制✅ · 分支✅(fork) · 重新生成 · 编辑 · 删除。
- **视觉**:Cherry 风格密度/圆角/分栏,复用现有 CSS 变量,保留 亮/暗 + i18n×3。

## 后端能力盘点(已 grep 确认)

可复用(无需新后端):`/v1/personas`(list)、`/v1/teams`、`/v1/sessions/{id}/persona|team`
(attach/detach)、`/v1/experts/suggest`、`/v1/sessions/{id}/fork`(=分支)、
`listSessions`、`listMessages`、`sendMessage`、`/status`、`/cancel`。

唯一缺口:**无消息 delete/edit 端点**(只有 fork)。`listSessions` 不返回所附 persona。

## 分阶段(每阶段单独 commit、可验收)

1. **后端切片**:`DELETE /v1/sessions/{id}/messages/{mid}`(owner-authed;删除/真·重新生成所需)。
   + shared client `deleteMessage`。唯一后端缺口。
2. **外壳 / IA**:导航栏(NavRail)+ 列表面板(AssistantTopicPanel:助手/话题 Tab)+
   重写 `App.tsx` 布局 + `styles.css` 新布局原语。助手列表接 personas/teams。
3. **顶栏模型/助手栏**:`ChatHeaderBar` —— 助手名 + 模型选择器上提;composer 下的
   `<select>` 移除/收起。
4. **消息操作条**:`MessageToolbar` —— 复制 / 重新生成(重发上条 user 输入)/
   编辑(改 user 文案重发)/ 分支(fork)/ 删除(调阶段 1 端点)。
5. **视觉打磨 + i18n×3 + 单测/e2e**:Cherry 观感细化;补 en/zh-CN/ja;
   vitest + Playwright 断言随新结构更新。

## v1 取舍(明确不做,后续再加)

- 新建 persona 仍跳 admin(创作 UI 在 admin-ui)。
- 「话题按助手分组」需后端给 `listSessions` 加 persona 字段 —— v1 先全列、由「当前
  助手」驱动新对话的 persona 绑定。
- 编辑用户消息 = 改文案后**重发为新轮**(append),非就地原位替换(就地替换需更多后端)。

## 约束

- 守 DEC-033:无新服务、仅 SQLite、单 owner。
- 风格遵循 `~/.claude/rules/*`:不可变更新、小文件、显式错误、边界校验。
- 不顺手改无关代码。
