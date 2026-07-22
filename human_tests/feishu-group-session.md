# 飞书群聊 Agent Session 真实场景测试

## 功能模块说明

验证飞书机器人进入群聊后持续接收并保存群消息，但默认只有 `@机器人`、`/g`、`/q` 或既有 slash 命令会触发处理；模型收到的是从上一次触发之后到本次触发为止的结构化多人对话。每个群使用独立 Session Key、上下文游标、Runner 和工作目录绑定。

## 前置条件

- macOS 或 Linux，已安装 Rust、Python 3、curl。
- 在仓库根目录执行。
- 测试使用临时 `BIFROST_DATA_DIR`、mock Feishu provider 和本地 mock Runner，不需要真实飞书凭证。

## 测试用例列表

### TC-FGS-01：普通群消息只保存、不触发模型

1. 启动临时 Bifrost。
2. 通过 `POST /_bifrost/api/im-gateway/debug/mock-inbound` 向 `chat-alpha` 注入 Alice、Bob 的两条 `chatType=group` 普通消息。
3. 等待 1 秒并检查 mock Runner 输入文件。

预期结果：Runner 输入文件不存在或为空；SQLite `im_group_messages` 中保存两条消息；非 owner 的 Bob 消息不会被拒绝。

### TC-FGS-02：艾特机器人触发结构化增量上下文

1. 在 TC-FGS-01 的消息之后注入 `@_user_1 请总结刚才的讨论`，并设置 `mentionBot=true`。
2. 等待 mock Runner 完成。
3. 读取 Runner 捕获的输入。

预期结果：只触发一次；这是该群第一次启动模型 Session，因此输入开头只包含解析后的群名称与群 ID。之后是 Alice/Bob 的两条背景对话和一条当前消息，每行均为 `<at id=飞书 open_id>显示名</at>：消息内容`；触发消息只出现一次。输入不包含 `provider_id`、Session Key、消息 ID、序号、时间戳、sender 类型、mentions 数组等内部字段。显示名不可用时使用 `<at id=飞书 open_id></at>`，不把 open_id 重复到标签正文，但 id 始终可用于后续准确 @ 原发送人。工具能力只由运行环境 Skill 提供，业务 Prompt 不指定具体工具。

### TC-FGS-03：slash 指令与单聊一致且系统指令不调用模型

1. 等待首轮任务结束，向同一群依次发送 `/cwd <仓库绝对路径>`、`/runner group-mock` 和 `/status`。
2. 检查 Runner 捕获条数。
3. 再发送普通补充消息和 `/g 继续给出行动项`。
4. 等待 Guide 对应任务结束，再发送 `/help extra`。

预期结果：三个受支持的系统命令不增加 Runner 调用，`/cwd` 和 `/runner` 只写当前群 binding；`/g` 按单聊的运行中 Guide/Queue 语义触发一次，只携带上一触发之后的有效群聊“补充消息”和本次引导消息，已处理的 `/cwd`、`/runner`、`/status` 不进入模型背景；后续输入不再重复群名称和群 ID。单聊不会识别的 `/help extra` 同样回落到模型，形成只含“谁发送了 `/help extra`”的一轮紧凑输入。

### TC-FGS-04：两次触发的上下文区间不重叠

1. 比较 TC-FGS-02 与 TC-FGS-03 捕获的两个输入。

预期结果：第二次输入不再包含第一次触发前的“发布窗口”等消息，但包含第一次触发后的新消息；无漏消息和重复区间。

### TC-FGS-05：不同群使用独立 Session

1. 向 `chat-beta` 注入一条普通消息和一条 `@机器人` 触发消息。
2. 检查第三次 Runner 输入和 SQLite `im_group_bindings`。

预期结果：第三次输入是 `chat-beta` 的首次模型输入，因此包含 Beta 群名称和群 ID，不含 `chat-alpha` 内容；数据库存在两个不同的群 Session Key 和独立游标，但这些内部 Session Key 不注入给模型。

### TC-FGS-06：重复事件幂等

1. 使用相同 `providerId + messageId` 重放群消息和触发消息。
2. 检查 `im_group_messages` 和 `im_group_turns` 唯一约束对应的记录数。

预期结果：消息只保存一份；同一触发消息只生成一个 Turn，不重复调用模型。

### TC-FGS-07：同一群内多个机器人按 Provider 独立

1. 在临时 Bifrost 管理端配置两个不同飞书应用的 Provider，并确认两个长连接都为 connected。
2. 把两个应用机器人加入同一个飞书群，先发送一条不 @ 的公共背景消息。
3. 只 @ 第一个机器人并让它复述背景，再只 @ 第二个机器人并执行不同任务。
4. 分别对两个机器人执行 `/status`，并检查 `admin/im_group_context.db` 中该 `chat_id` 的 binding。

预期结果：每次只有被 @ 且 `open_id` 匹配当前 Provider 的机器人响应；两个 Session Key 分别为 `im:<provider-a>:group:<chat-id>` 和 `im:<provider-b>:group:<chat-id>`，各自第一次启动模型时的 Prompt 均包含相同群的群 ID、群名称以及可复用的发送者 `<at>`；后续 Prompt 只保留有效背景和当前消息，不重复基本信息。上下文游标、对话历史、Runner 与工作目录互不串用；机器人重名时仍按 `open_id` 精确区分。

### TC-FGS-08：飞书卡片无顶部标题并原生引用触发消息

1. 通过本地 mock Feishu OpenAPI 调用普通回复卡片和 CardKit 进度卡片发送链路，记录每次请求的路径和请求体。
2. 检查普通回复、进度卡片首次发送和后续 Turn rollover 的卡片 JSON。
3. 在真实飞书群中 @ 机器人并执行 slash 指令，观察返回卡片的视觉样式和引用关系。

预期结果：所有 Bifrost 生成的飞书卡片都不包含根级 `header`，不显示大块彩色顶部标题；任务计划和工具记录等内部折叠面板标题仍可见。存在来源消息时，请求路径为 `/im/v1/messages/{触发 message_id}/reply`，请求体不携带 `receive_id`，飞书客户端显示轻量原生引用；后续 Turn 的进度卡片引用本轮最新触发消息。上线通知、定时任务等无来源消息场景可直接发送，但也没有顶部标题。

## 执行方式

TC-FGS-01 至 TC-FGS-05 由以下真实服务脚本逐条执行：

```bash
bash e2e-tests/tests/test_feishu_group_session_context.sh
```

TC-FGS-06 的账本和 Turn 幂等边界执行：

```bash
cargo test -p bifrost-admin group_store_deduplicates_messages_and_triggers -- --nocapture
```

TC-FGS-07 使用临时数据目录启动真实管理端，由测试者绑定两个真实飞书 Provider 后在同一群逐项执行，并结合 SQLite binding 记录核验。

TC-FGS-08 的普通卡片、CardKit 进度回复路径和 Provider 根级标题兜底执行：

```bash
cargo test -p bifrost-admin feishu_message_cards_use_native_reply_paths_and_strip_root_headers -- --nocapture
```

## 清理步骤

脚本退出时停止临时 Bifrost 进程并删除 `mktemp` 数据目录；设置 `KEEP_TEST_DIR=true` 可保留现场用于检查 SQLite 与日志。
