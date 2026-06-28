# 飞书（Lark）对接 runbook — 现场 demo 指南

把 xiaoguai 接进飞书：飞书里 @机器人 → xiaoguai 跑一轮 ReactAgent → 回到飞书群里回复。
本手册聚焦**现场 demo**（用隧道把本机 `:7600` 暴露公网），但 prod 部署的差异也在踩坑章节标注。

> **单 owner 部署（DEC-033）。** xiaoguai 是一个自包含的 Rust 单二进制
> （`xiaoguai serve`），内嵌 SQLite —— 没有 Postgres / Redis / 外部队列。
> 飞书凭据通过 `XIAOGUAI_IM_FEISHU__*` 环境变量注入，**不要**写进
> `config.yaml` 或提交到仓库。没有租户、没有团队，只有一个隐式 owner。

代码事实（本手册所有路径/变量名/签名算法均已对代码核对，不是凭记忆）：

| 项 | 值 | 出处 |
|---|---|---|
| Webhook 路由 | `POST /v1/im/feishu/webhook` | `crates/xiaoguai-im-gateway/src/router.rs` `mount_feishu_with_history` → `mount_with_history("/v1/im/feishu/webhook", …)`，在 `serve` 里以 `.merge()` 挂到根，**无前缀** |
| 必需 env（启用路由） | `XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN` | `crates/xiaoguai-core/src/lib.rs` `build_feishu_gateway`；未设或为空 → 返回 `None`，**整个飞书路由不挂载** |
| 回复 env（可选） | `XIAOGUAI_IM_FEISHU__APP_ID` + `XIAOGUAI_IM_FEISHU__APP_SECRET` | 同上；任一缺失 → 入站照常处理，但**回复走 stub**（只记日志，不真发） |
| 默认端口 | `:7600` | DEC-033 |
| 签名 header | `X-Lark-Signature` / `X-Lark-Request-Timestamp` / `X-Lark-Request-Nonce` | `crates/xiaoguai-im-feishu/src/lib.rs` `verify` |
| Feishu OpenAPI 基址 | `https://open.feishu.cn` | `crates/xiaoguai-im-feishu/src/api.rs` `DEFAULT_BASE_URL` |

> **`VERIFICATION_TOKEN` 这个名字有坑。** 它在代码里被当成飞书的
> **「Encrypt Key / 加密 Key」**（签名密钥）使用 —— 见下面「签名验证」一节。
> 飞书后台同时还有一个叫 "Verification Token" 的字段；**xiaoguai 用的是 Encrypt Key**，
> 别填错。环境变量沿用了历史命名，含义以代码为准。

---

## 0. 你需要准备的东西

- [ ] 一个飞书账号，能进 [飞书开放平台](https://open.feishu.cn/app)（企业自建应用入口）
- [ ] 本机能跑 `xiaoguai serve`（pip 安装或源码 build 均可），监听 `:7600`
- [ ] 一个能把 `:7600` 暴露成公网 HTTPS 的隧道工具：`cloudflared` 或 `ngrok`
      （飞书事件订阅**必须是公网可达的 HTTPS URL**，localhost 不行）

---

## 1. 飞书开放平台：建应用、拿凭据、加权限、配事件订阅

### 1.1 创建企业自建应用

1. 打开 <https://open.feishu.cn/app> → 点 **「创建企业自建应用」**。
2. 填**应用名称**（如 `xiaoguai-demo`）、图标、描述 → **「创建」**。
3. 进入应用后，左侧菜单 **「凭证与基础信息」**，记下：
   - **App ID**（形如 `cli_axxxxxxxxxxxxxxx`）→ 这是 `XIAOGUAI_IM_FEISHU__APP_ID`
   - **App Secret**（点「查看」/「重置」获取）→ 这是 `XIAOGUAI_IM_FEISHU__APP_SECRET`

### 1.2 拿 Encrypt Key（= `VERIFICATION_TOKEN` 环境变量）

1. 左侧 **「事件与回调」 → 「事件配置」**（部分版本叫「事件订阅」）。
2. 找到 **「加密策略」 / "Encrypt Key"** 一栏：
   - 点 **「生成」**（或填一段你自己的随机串，例如 `openssl rand -hex 32`），
     得到 **Encrypt Key**。
   - **复制它** → 这就是 `XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN` 的值。
   - ⚠️ xiaoguai 当前实现**要求**启用 Encrypt Key（签名校验依赖它）。同页那个
     "Verification Token" 字段 xiaoguai 不消费，留着即可。

### 1.3 添加机器人能力 + `im:message` 权限

1. 左侧 **「添加应用能力」** → 找到 **「机器人」** → **「添加」**（这样应用才能在群里发消息、被 @）。
2. 左侧 **「权限管理」**，搜索并勾选至少这些权限（scope）：
   - **`im:message`**（读写消息，收 @ 必需）
   - **`im:message:send_as_bot`** / **「以应用的身份发消息」**（发回复必需）
   - 如需读群信息可加 `im:chat`（可选）。
3. 勾完后页面顶部会提示**需要发布版本**才能生效 —— 见 1.5。

### 1.4 配置事件订阅 webhook（先把 xiaoguai 跑起来 + 隧道开好再回填，见第 2-3 节）

1. 回到 **「事件与回调」 → 「事件配置」**。
2. 订阅方式选 **「将事件发送至开发者服务器」**（即 webhook 推送，不是长连接）。
3. **「请求地址 URL」** 这个框里填：
   ```
   https://<你的公网域名>/v1/im/feishu/webhook
   ```
   例如用 cloudflared 拿到的 `https://abc-def.trycloudflare.com/v1/im/feishu/webhook`。
   - ⚠️ 路径**必须是** `/v1/im/feishu/webhook`（一字不差）。
   - 此刻先别点保存 —— 飞书一保存就会立刻发 challenge 验证（第 4 节），
     所以要等 xiaoguai + 隧道都 ready。
4. 在 **「添加事件」** 里订阅 **「接收消息」**（`im.message.receive_v1`）—— 这是用户
   发消息/ @机器人 时飞书推给你的事件。

### 1.5 发布版本

权限和能力改动需要发版才生效：左侧 **「版本管理与发布」 → 「创建版本」** →
填版本号和说明 → 提交。企业自建应用通常可由管理员**自助审核通过**。
（demo 时如果是你自己的测试企业，一般秒过。）

---

## 2. xiaoguai 侧：配 env + 启动

把三个变量喂给进程。**最简单的 demo 做法**（当前 shell 临时变量）：

```bash
export XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN='<上面 1.2 的 Encrypt Key>'
export XIAOGUAI_IM_FEISHU__APP_ID='cli_axxxxxxxxxxxxxxx'      # 1.1 的 App ID
export XIAOGUAI_IM_FEISHU__APP_SECRET='<1.1 的 App Secret>'

xiaoguai serve --host 0.0.0.0 --port 7600
```

- `--host 0.0.0.0` 让隧道/局域网能连到（默认只 bind 本地回环时隧道连不上）。
  端口默认就是 `7600`，`--port` 可省。
- 启动后留意日志：
  - **没有** `XIAOGUAI_IM_FEISHU__APP_ID / __APP_SECRET unset` 这条 warn →
    说明回复走真实 OpenAPI（你想要的 demo 效果）。
  - 如果只配了 `VERIFICATION_TOKEN`、没配 App ID/Secret，会看到：
    `serve: XIAOGUAI_IM_FEISHU__APP_ID / __APP_SECRET unset — Feishu replies will be stubbed`
    —— 入站能收、能解析、能跑 agent，但**不会真的发回飞书**（reply 被 stub 吞掉）。
    demo 想看到机器人真回话，**必须**把 App ID/Secret 都配上。

> **prod 做法**（非 demo）：把这三个变量放进 systemd drop-in 的 `EnvironmentFile`
> （`chmod 600`，owner 持有），不要导进交互式 shell。参见
> `docs/runbooks/im-adapter-onboarding.md` 的 "Configure: environment variables" 一节。

**快速自检**（确认进程起来了、路由挂上了）：

```bash
curl -s http://127.0.0.1:7600/healthz        # 应返回健康响应
# 直接打飞书路由（无签名）应得 401 —— 证明路由已挂载且签名校验生效：
curl -s -o /dev/null -w '%{http_code}\n' -XPOST http://127.0.0.1:7600/v1/im/feishu/webhook
# 期望输出: 401
```

如果上面那条返回的是 404 而不是 401，说明 `VERIFICATION_TOKEN` 没设/为空 →
`build_feishu_gateway` 返回了 `None`，**路由根本没挂**。回到第 2 节检查变量。

---

## 3. 把 `:7600` 暴露公网（demo 用隧道）

飞书事件订阅必须 HTTPS 公网可达。二选一：

### 方案 A：cloudflared（无需账号，最快）

```bash
# 装：brew install cloudflared  (macOS) / 见官方 release
cloudflared tunnel --url http://localhost:7600
```

它会打印一个 `https://<随机>.trycloudflare.com` 的临时域名。把
`https://<随机>.trycloudflare.com/v1/im/feishu/webhook` 填回 1.4 的「请求地址 URL」。

### 方案 B：ngrok

```bash
ngrok http 7600
```

取 `Forwarding` 里的 `https://<随机>.ngrok-free.app`，拼上 `/v1/im/feishu/webhook` 回填。

> ⚠️ **隧道域名每次重启会变**（免费档）。变了就要回飞书后台重新填 URL 并重新过
> challenge。demo 前现开隧道、当场填，别提前一天开。
> ⚠️ 隧道必须指向 `localhost:7600` 且 xiaoguai 用 `--host 0.0.0.0` 起，
> 否则隧道连不到进程（表现为飞书报「请求地址无法访问」）。

---

## 4. challenge 首验（保存 URL 时飞书会做的事）

当你在 1.4 点**保存「请求地址 URL」**时，飞书会立刻向该 URL POST 一个
**url_verification** 包，形如：

```json
{ "type": "url_verification", "challenge": "<一段随机串>", "token": "..." }
```

xiaoguai 的处理（`crates/xiaoguai-im-feishu/src/lib.rs`）：

1. **先验签**，再看 body —— challenge 包也走签名校验（防止未认证的人探测 challenge 路径）。
   所以 Encrypt Key 必须和飞书后台一致，否则 challenge 直接 401，飞书报「校验失败」。
2. 验签过后，`parse_event` 看到顶层有 `challenge` 字段 → 返回 `ImEvent::Challenge`。
3. 路由层**同步**回 `{"challenge":"<原样回显>"}`（`router.rs` `handle_webhook` 的
   `ImEvent::Challenge` 分支）。

飞书收到回显的 `challenge` 与它发出的一致 → **URL 校验通过**，订阅生效。

**如果这一步失败**，对照「踩坑」第 1、3、4 条。

---

## 5. 测试：飞书里 @机器人 → 看日志 + 回复

1. 在飞书里**把机器人拉进一个群**（或与机器人建单聊；当前实现按 `chat_id` 路由，
   群聊单聊都走同一条 webhook）。
2. 在群里 **@机器人** 并发一句话，例如 `@xiaoguai-demo 你好`。
3. **看 xiaoguai 日志**，应能看到这条链路（关键字）：
   - 入站计数 / `spawn_agent_reply`（开始处理）
   - `im reply sent`（带 `provider=feishu`、`chat_id=oc_...`、`len=...`）——
     说明回复已通过 OpenAPI 发出。
   - 若看到 `im reply failed` → 多半是 App ID/Secret 错、权限没发版、或网络。
4. **回到飞书群**，机器人应回一条消息（内容是 agent 这一轮的输出）。

> 行为说明（已对代码核对，`router.rs`）：收到消息后 xiaoguai **先同步回 HTTP 200
> `{"status":"accepted"}`**（避免阻塞飞书、飞书超时会重投），**再在后台**跑
> `ReactAgent::run_to_completion`，完成后调 `provider.reply(...)` 把结果发回飞书。
> 所以「飞书里看到回复」会比「日志里收到消息」晚一会儿（取决于模型耗时），这是正常的。

---

## 6. 踩坑（demo 前逐条过一遍）

1. **签名校验失败（飞书报「校验失败」/ 401）。**
   xiaoguai 用的签名是
   `sha256(timestamp + nonce + encrypt_key + body).hex`（events v2，启用 Encrypt Key），
   走 `X-Lark-Signature`。
   - 最常见原因：`XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN` 填的不是飞书后台那个
     **Encrypt Key**（填成了别的字段、或前后有空格/换行）。两边必须**逐字一致**。
   - 改了 env 必须**重启** `serve` 才生效。

2. **challenge 过不了（保存 URL 时飞书报错）。**
   - URL 路径不是 `/v1/im/feishu/webhook`（写错/漏了 `/v1`/多了前缀）。
   - 隧道没起 / 域名过期 / xiaoguai 没在 `0.0.0.0:7600` 监听 → 飞书打不到。
   - Encrypt Key 不一致（见第 1 条）—— challenge 也要先验签。

3. **公网不可达（飞书「请求地址无法访问」）。**
   - `xiaoguai serve` 忘了 `--host 0.0.0.0`，只 bind 了回环。
   - 隧道指向了错误端口（不是 7600）。
   - 本机防火墙挡了隧道到 7600 的连接。
   - 自检：在另一台机器/手机流量上 `curl -i https://<隧道域名>/v1/im/feishu/webhook`
     无签名应得 **401**（不是连接超时、不是 404）。

4. **路由返回 404 而不是 401。**
   说明 `VERIFICATION_TOKEN` 未设或为空 → `build_feishu_gateway` 返回 `None`，
   飞书路由**没挂载**。设上变量、重启。

5. **机器人收到了但不回话（日志有入站、没 `im reply sent`，或有 stub 日志）。**
   - 看到 `… __APP_ID / __APP_SECRET unset — Feishu replies will be stubbed`：
     App ID/Secret 没配齐 → 回复走 stub，**不会真发**。demo 必须两个都配。
   - 看到 `im reply failed` / `feishu send error code=...`：App Secret 错、
     权限（`im:message` / 发消息）没**发版**生效、或 chat 不允许机器人发言。
   - 权限改了一定要在「版本管理与发布」**发新版本**，否则线上仍是旧权限。

6. **token 缓存语义（一般不用管，了解即可）。**
   xiaoguai 进程内缓存一个 `tenant_access_token`（`api.rs` `TokenCache`），
   过期前 60s 单飞刷新。
   - 你**重置了 App Secret** 后，旧 token 直到过期或进程重启才换 →
     现象是改密后短时间内回复仍可能用旧 token 失败。最稳的做法：**改密后重启 `serve`**。
   - token 获取失败（`feishu auth error code=...`）通常是 App ID/Secret 不匹配。

---

## 7. 一页纸 demo checklist（按顺序）

1. 开放平台建应用 → 记 **App ID / App Secret**（1.1）。
2. 事件配置里**生成 Encrypt Key**（1.2）。
3. 加**机器人能力** + 勾 **`im:message` / 发消息**权限（1.3）。
4. 三个 env 设上（`VERIFICATION_TOKEN`=Encrypt Key、`APP_ID`、`APP_SECRET`），
   `xiaoguai serve --host 0.0.0.0 --port 7600`（第 2 节）。
5. `curl … /v1/im/feishu/webhook` 应得 **401**（路由挂上了）。
6. 开隧道 `cloudflared tunnel --url http://localhost:7600`（第 3 节）。
7. 事件配置「请求地址 URL」填 `https://<隧道域名>/v1/im/feishu/webhook` → 保存 →
   **challenge 自动通过**（第 4 节）。
8. 订阅「接收消息」事件 → **发布版本**（1.5）。
9. 把机器人拉进群 → **@它**发一句 → 看日志 `im reply sent` → 飞书里看到回复（第 5 节）。

---

## 附：本地端到端验证（不连真飞书，已随仓库 ship）

无需真飞书也能证明 parse / challenge / 签名 / 路由 / reply 这条链是通的 ——
集成测试在 `crates/xiaoguai-im-gateway/tests/feishu_route.rs`，用
`tower::ServiceExt::oneshot` 直接打 `POST /v1/im/feishu/webhook`：

- `challenge_round_trips` —— 带正确签名的 `{"challenge":"hello"}` → 断言响应回显 `challenge`。
- `message_event_is_accepted_and_reply_dispatched` —— 带正确签名/`event_id` 的消息事件 →
  断言 200 `accepted`，且后台 ReactAgent（跑 `MockBackend`）产出的 reply 落进 recording sink。
- `missing_signature_yields_401` / `malformed_signed_body_yields_400` —— 负路径。
- `conversation_history_accumulates_per_chat` —— 同一 `chat_id` 多轮累积历史、不同 chat 互相隔离。

签名/token 缓存的单元测试在 `crates/xiaoguai-im-feishu/src/lib.rs` 与 `src/api.rs`。

跑：

```bash
cargo test -p xiaoguai-im-gateway      # 含 feishu_route + 其它适配器
cargo test -p xiaoguai-im-feishu       # 签名 + token 缓存单测
```
