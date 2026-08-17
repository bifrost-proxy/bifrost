# 飞书 Slash 命令选择卡片

## 背景

IM external runner 已支持以下文本命令：

- `/resume` 列出最近 20 个 Codex、Traex 或 Claude Code 本地 session，
  `/resume <session-id>` 选择待恢复会话。
- `/model` 查看当前模型，`/model <slug>` 设置当前 Bifrost session 的模型。
- `/effort` 查看当前 Reasoning Effort，`/effort <level>` 设置当前 Bifrost
  session 的推理强度。

列表结果只返回 Markdown，用户必须复制 session id、模型 slug 或 effort level，再粘贴为
下一条命令。飞书卡片已使用 JSON 2.0，支持按钮 `behaviors.callback.value` 和
`card.action.trigger` 长连接回调，因此可以把候选项直接渲染为按钮，并把点击动作重新送入
原 slash 命令处理链路。

## 用户目标验证清单

### 必须实现

- 飞书中发送 `/resume` 后，卡片展示最近最多 20 个本地 session 及对应选择按钮。
- 飞书中发送 `/model` 后，卡片展示当前状态、可用模型及模型选择按钮。
- 飞书中发送 `/effort` 后，卡片展示当前状态、当前模型支持的推理强度及选择按钮。
- 点击按钮后直接执行对应的 `/resume <id>`、`/model <slug>` 或
  `/effort <level>`，不要求复制粘贴。
- 模型和推理强度卡片提供“恢复 Runner 默认值”按钮，分别复用 `/model clear` 和
  `/effort clear`。

### 必须不破坏

- `/resume <id>`、`/model <slug>`、`/effort <level>` 文本命令保持兼容。
- `/models`、`/efforts` 及 Web Chat 继续使用现有文本结果，不强制改成飞书卡片。
- Weixin、WeChat 和不具备卡片回调能力的 provider 保持现有 Markdown 回复。
- 按钮点击必须经过原有 busy 状态、Runner、adapter、catalog、session state 和本地
  session 校验，不新增旁路写状态接口。
- 回调不能执行 `/stop`、`/clear`、普通 prompt 或任意开发者未允许的命令。

### 必须真实验证

- 使用 JSON 2.0 卡片结构验证每个按钮包含唯一 `element_id` 和
  `behaviors=[{"type":"callback","value":...}]`。
- 使用真实 `card.action.trigger` 结构验证单聊和群聊点击都回到原 session。
- 使用真实 Bifrost 临时服务、mock Feishu callback 和隔离 provider home 验证
  `/resume`、`/model`、`/effort` 的 list → click → state 更新闭环。
- 验证非原请求用户、错误 chat、错误 provider、过期卡片和任意命令均不能修改状态。

## 交互与卡片结构

卡片使用无根级标题的 JSON 2.0 结构。候选项渲染为单个 `select_static` 下拉，避免选项
过多时平铺按钮刷屏。每个 option 的 `value` 是【字符串】，承载序列化后的完整绑定（飞书
`select_static` 要求 option value 为唯一字符串，各 option 的 `command` 天然不同保证唯一）：

```json
{
  "schema": "2.0",
  "config": {
    "width_mode": "fill",
    "update_multi": true
  },
  "body": {
    "elements": [
      {
        "tag": "markdown",
        "element_id": "choice_summary",
        "content": "当前状态和候选项"
      },
      {
        "tag": "select_static",
        "element_id": "choice_select",
        "width": "fill",
        "placeholder": { "tag": "plain_text", "content": "请选择…" },
        "behaviors": [
          { "type": "callback", "value": { "bifrostAction": "slash_choice" } }
        ],
        "options": [
          {
            "text": { "tag": "plain_text", "content": "候选项" },
            "value": "{\"bifrostAction\":\"slash_choice\",\"providerId\":\"feishu-main\",\"chatId\":\"oc_xxx\",\"chatType\":\"p2p\",\"userId\":\"ou_xxx\",\"command\":\"/model example-model\",\"expiresAtMs\":0}"
          }
        ]
      }
    ]
  }
}
```

`/resume` 下拉首个 option 固定为「🆕 新建会话」（`/resume new`），选择后清空 session 的
`external_thread_id` / `external_conversation_id`，下一条普通消息开启全新会话；其后是最多
20 个本地 session。`/model` 沿用最多 41 个 option（含 `/model clear`）；`/effort` 按当前
模型目录或 Runner 兼容默认值生成，含 `/effort clear`。为兼容下拉迁移前已发出的旧卡片，
回调解析同时接受 `select_static`（选中值在 `action.option` 字符串）与遗留 `button`
（绑定对象在 `action.value`）两种结构。
                  "tag": "plain_text",
                  "content": "候选项"
                },
                "behaviors": [
                  {
                    "type": "callback",
                    "value": {
                      "bifrostAction": "slash_choice",
                      "providerId": "feishu-main",
                      "chatId": "oc_xxx",
                      "chatType": "p2p",
                      "userId": "ou_xxx",
                      "command": "/model example-model",
                      "expiresAtMs": 0
                    }
                  }
                ]
              }
            ]
          }
        ]
      }
    ]
  }
}
```

> 说明：上方 `column_set` + `button` 是【迁移前】的平铺结构，仅为向后兼容旧卡片保留；
> 当前所有选择卡片都改用前述单个 `select_static` 下拉，回调绑定语义完全一致。

## 回调归一化与安全边界

飞书回调使用以下字段（`select_static` 与遗留 `button` 通用）：

- `header.event_type=card.action.trigger`
- `event.operator.open_id`
- `event.action.tag`（`select_static` 或遗留 `button`）
- `event.action.option`（`select_static` 选中项字符串）或 `event.action.value`（`button` 绑定对象）
- `event.context.open_message_id`
- `event.context.open_chat_id`

Bifrost 在 Feishu provider 边界把合法回调归一化为一条 `message.receive` 风格的内部
事件，`message.text` 为受控 slash 命令。事件随后进入既有 event loop，不直接修改
session state。

归一化必须同时满足：

1. `bifrostAction` 固定为 `slash_choice`。
2. `providerId` 与当前长连接 provider 一致。
3. `operator.open_id` 与卡片绑定的 `userId` 一致。
4. `context.open_chat_id` 与卡片绑定的 `chatId` 一致。
5. `chatType` 只能是 `p2p` 或 `group`。
6. `expiresAtMs` 未过期；选择卡片有效期为 24 小时。
7. `command` 只能解析为：
   - `/resume <id>`
   - `/model <slug>` 或 `/model clear`
   - `/effort <level>` 或 `/effort clear`

回调事件不把卡片消息的 `open_message_id` 作为内部 `source.message_id`，避免同一张卡片
的不同按钮被消息级去重误判为重复；去重使用每次 callback 自带的 `header.event_id`。

## 飞书应用配置

飞书应用必须在开发者后台订阅新版 **卡片回传交互**
`card.action.trigger`，并使用与消息事件相同的长连接接收方式。未订阅时卡片仍可展示，
但客户端点击会返回平台侧“未配置卡片回调”错误。Bifrost 收到回调后先通过 WebSocket
协议 ACK，再在异步 event loop 中读取模型目录或本地 session，避免阻塞飞书的 3 秒回调
响应要求。

## 实现边界

- `crates/bifrost-admin/src/im_gateway/feishu_card_action.rs`
  - 构造 JSON 2.0 选择卡片。
  - 解析并校验 `card.action.trigger`。
  - 将合法点击归一化为 `ImEvent`。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_choice_card.rs`
  - 构造回复目标、发送卡片、记录 outbound message log。
  - 发送失败时回退到原 Markdown 卡片。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`
  - 为三个无参数命令收集结构化候选项。
  - 选择动作继续复用原命令处理逻辑。
- `crates/bifrost-admin/src/handlers/im_gateway/debug_inbound.rs`
  - 提供隔离 E2E 所需的 mock Feishu callback 注入入口。

## 测试方案

### 单元测试

- `feishu_choice_card_builds_two_column_callback_buttons`
  - 断言 JSON 2.0、按钮数量、唯一 `element_id`、callback value 和 24 小时有效期。
- `feishu_card_action_normalizes_authorized_group_and_p2p_clicks`
  - 断言 operator/chat/provider 绑定与内部 slash 文本。
- `feishu_card_action_rejects_unauthorized_expired_or_arbitrary_commands`
  - 覆盖其他用户、其他 chat、其他 provider、过期、`/stop`、无参数命令和坏结构。
- command handler 测试覆盖 `/resume`、`/model`、`/effort` 生成候选卡片，并验证
  `/model clear`、`/effort clear` 仍走原持久化逻辑。

### E2E

新增 `e2e-tests/tests/test_feishu_slash_choice_cards.sh`：

1. 使用临时数据目录、动态端口、隔离 provider home 和 mock runner 启动最新 Bifrost。
2. 注入 `/resume`、`/model`、`/effort` 飞书消息并读取 mock Feishu 卡片请求。
3. 从卡片提取 callback value，构造真实 `card.action.trigger` 后注入。
4. 断言 session state 分别写入完整本地 session id、model override 和 effort override。
5. 使用错误用户、错误 chat、过期值和任意命令回调，断言状态不变。

启动必须设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和
`BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1`，使用 `--no-system-proxy` 和动态端口，
退出时只按本脚本记录的 PID 清理。

### human_tests

更新 `human_tests/local-session-resume.md`，新增飞书 `/resume`、`/model`、`/effort`
选择卡片真实链路、单聊/群聊绑定、越权点击和文本命令回退用例；更新
`human_tests/readme.md` 对应模块索引后立即逐条执行。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照用户目标复核三个命令是否都返回按钮。
- review callback allowlist、用户/chat/provider 绑定、过期和 dedup 边界。
- 执行 `git status --short`、`git diff`、相关 Rust 单测和新增 E2E。
- 发现问题立即修复并复跑失败路径。

### 第 2 轮

- 基于最新 diff 复查文本命令、非飞书 provider、busy session、群聊 session key 和文档。
- 检查 `human_tests/readme.md` 索引、测试启动护栏、临时目录和进程清理。
- 复跑相关 Rust 单测、E2E、human tests 和 `make coverage-changed`。
- 第 2 轮仍发现问题时继续追加 Review/Fix/Test 轮次。

## 校验要求

- 先执行新增 E2E 和 `human_tests`。
- 修改生产 Rust 后执行 `make coverage-changed`，changed-lines 覆盖率必须达到 95%；
  远端 CI 必须通过 unit + E2E 聚合覆盖率 90% 棘轮门禁。
- 最后执行 rust-project-validate，包括 fmt、clippy、相关测试、build 和
  `cargo test --workspace --all-features`。
- 按修改范围执行 `bash scripts/ci/local-ci.sh --e2e-only shell`。
