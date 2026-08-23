# 飞书 Bot 指令菜单

## 功能目标

Bifrost 为飞书 Bot 提供一个内置的两级指令菜单。菜单只作为现有 IM slash command 的输入界面：用户点击菜单后，Bifrost 将受信任的 `event_key` 映射为固定 slash command，再交给现有 Owner 校验、会话解析、命令分发、队列和回复链路。菜单不能携带或执行任意命令。

默认菜单使用三个一级分组：

| 分组 | 菜单项 | Canonical command |
| --- | --- | --- |
| 会话 | 状态 | `/status` |
| 会话 | 恢复会话 | `/resume` |
| 会话 | 排队状态 | `/q` |
| 会话 | 停止任务 | `/stop` |
| Agent | 选择模型 | `/model` |
| Agent | 选择 Runner | `/runner` |
| Agent | 推理强度 | `/effort` |
| Agent | Fast 模式 | `/fast status` |
| 工具 | 当前目录 | `/pwd` |
| 工具 | 使用帮助 | `/help` |

`/clear` 和 `/reset` 不进入默认菜单，避免误触造成上下文丢失。`/fast` 不使用无参数形式，因为无参数命令会立即切换状态；菜单入口先查询状态，再通过选择卡片执行 `/fast on` 或 `/fast off`。

## 数据面设计

飞书通过 `application.bot.menu_v6` 推送菜单点击事件。事件仅包含操作者、`event_key` 和时间戳，不包含消息或群聊 ID。Bifrost 将其规范化为私聊虚拟消息：

```text
application.bot.menu_v6
  -> 固定 event_key allowlist
  -> message.receive
  -> provider_id + operator.open_id 私聊会话
  -> 现有 IM command dispatcher
```

规范化后的 `source.user_id` 为操作者 `open_id`，`source.chat_id` 为空，`source.chat_type` 为 `p2p`。因此菜单只能操作点击者与 Bot 的私聊会话，不能猜测或控制最近使用的群聊、话题或其他会话。

选择卡片绑定必须支持 `chat_id` 可选：

- `provider_id` 和 `user_id` 始终必填；
- 缺少 `chat_id` 时只允许 `p2p`，卡片通过 `open_id` 发送；
- 回调仍校验 provider、操作者、有效期和命令 allowlist；
- 若原绑定有 `chat_id`，继续严格校验回调的 `open_chat_id`；
- 若原绑定没有 `chat_id`，回调携带的真实 `open_chat_id` 成为后续消息的 chat 绑定。

## 控制面设计

`FeishuAppProvisioner` 负责把 Bifrost 期望状态同步到飞书 Application v7：

1. `PATCH /application/v7/applications/{app_id}/ability`，只写 `bot_menu_enable` 与 `bot_menus`。
2. `PATCH /application/v7/applications/{app_id}/config`，只向 `event.add_events` 增加 `application.bot.menu_v6`。
3. 仅在显式请求时调用 `POST /application/v7/applications/{app_id}/publish`。

不得覆盖 Bot enable、菜单展示策略、事件订阅方式、Webhook 地址或其他应用能力。发布时必须显式给出移动端和 PC 默认能力；默认值为 `bot`，但导入应用不自动发布。

本地保存目标 App 身份与 canonical desired state 的 SHA-256 digest、最近成功阶段、版本和错误信息。digest 相同且已经完成相同发布目标时直接跳过远端写操作；provider ID 不变但 App ID 或平台 base URL 变化时必须重新应用，凭据本身不进入 digest。

## 连接生命周期

- 手工 `provider connect`：连接前自动 reconcile 菜单 draft；失败记录 warning，但不阻止已有 Bot 建立 WebSocket。
- QR setup 首次创建：尝试 reconcile 并 publish，然后连接；如果 Application v7 不支持 `PersonalAgent`，记录 `unsupported_app_type` 并继续连接。
- 服务启动恢复：每个已启用且允许事件连接的历史 Feishu Provider 在建立 WebSocket 前执行一次 draft reconcile；digest 已匹配时跳过远端写入，失败只记录状态和 warning，不阻断连接。
- WebSocket reconnect：只恢复传输连接，不执行 PATCH 或 publish。
- CLI/API 显式 sync：按请求执行 draft sync 或 sync + publish。

Application v7 官方约束只保证开发者后台创建的自建应用可用。QR setup 当前使用 `PersonalAgent`，因此平台拒绝必须作为可诊断降级，不得让消息连接失败。

## Admin API 与 CLI

Admin API：

```text
GET  /_bifrost/api/im-gateway/providers/{id}/feishu/menu/preview
GET  /_bifrost/api/im-gateway/providers/{id}/feishu/menu/status
POST /_bifrost/api/im-gateway/providers/{id}/feishu/menu/sync
```

CLI：

```text
bifrost im provider menu <provider> preview
bifrost im provider menu <provider> status
bifrost im provider menu <provider> sync
bifrost im provider menu <provider> sync --publish
```

## 依赖与错误模型

实现复用现有 `reqwest` client、tenant token cache、provider secret、消息事件循环和选择卡片。Provision 错误包含稳定的 `stage`、错误分类、飞书业务码、HTTP 状态和 `x-tt-logid` request ID；不得暴露 app secret 或 tenant token。

错误分类至少包括：provider 不存在、provider 类型错误、凭证缺失、菜单校验失败、ability 更新失败、事件配置失败、发布失败、应用审核中和平台不支持的应用类型。

## 验证方案

### 单元与 HTTP contract

- 默认菜单满足两级、一级不超过 3、二级不超过 5、ID 唯一、父节点存在、event key 全部位于 allowlist。
- `application.bot.menu_v6` 正确提取操作者并映射 command；未知 key、缺少操作者或缺少事件 ID时拒绝。
- 菜单事件复用私聊 session 与 Owner 过滤。
- 无初始 `chat_id` 的选择卡片向 `open_id` 发送，回调仍拒绝跨用户、跨 provider、过期和任意命令。
- Fake Feishu 断言 ability/config/publish 的 method、path 和最小 body；覆盖业务错误码、request ID、审核中和不支持应用类型。
- Admin API 与 CLI 覆盖 preview/status/sync、`--publish` 和非法参数。

### E2E

在 `e2e-tests/tests/` 使用动态端口、临时数据目录和 loopback Fake Feishu：

- 创建测试 provider 并执行 CLI preview/sync；
- 断言 token、ability 与 config 请求；
- 注入菜单事件并验证 `/status` 经过现有事件循环产生回复；
- 验证 Runner/Fast 选择卡片、未知 key 和非 Owner 拒绝；
- 预存历史 Provider 后启动服务，确认首次启动自动同步 draft，使用同一数据目录再次启动时 digest 幂等跳过。
- 确认 WebSocket reconnect 不触发第二次 provision。

### 真实场景

`human_tests/feishu-bot-menu.md` 使用专用测试应用验证菜单显示、点击命令、Owner 边界、选择卡片、发布和重连幂等。不得修改未明确作为测试目标的生产 Bot；没有专用应用授权时，将该外部步骤记录为阻塞，Fake Feishu E2E 仍必须通过。

### Rust 门禁

先执行相关 `bifrost-admin` / `bifrost-cli` 测试和 E2E，再执行 `make coverage-changed`、workspace fmt、clippy、all-features tests/build 及适用 local CI。

## Review/Fix/Test 闭环

第一轮复核菜单到 command 的安全边界、私聊 session 语义、控制面最小 PATCH、publish 策略、历史 Provider 启动恢复和 reconnect 行为，修复后复跑相关单元测试与 Fake Feishu E2E。

第二轮复查最新 diff、CLI help、Admin 协议、设计文档、human test 索引和 changed-lines 覆盖率，修复后复跑受影响测试。若仍发现缺陷或失败，继续追加 review 轮次。
