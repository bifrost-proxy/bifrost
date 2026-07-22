# 飞书群聊 Agent Session

## 目标

飞书机器人加入群聊后持续接收并保存群内消息，但默认不因普通聊天调用模型。只有以下输入会进入现有 IM Agent 命令或执行链路：

- 消息明确 `@` 当前机器人；
- 显式 slash 指令；
- 运行中通过 `/g <内容>` 发出的 Guide。

每个 `(provider_id, chat_id)` 对应一个独立 Session、消息游标和工作目录。群聊 slash 指令复用单聊的解析、响应和 Runner 行为，不维护第二套命令协议。

## 会话和增量区间

- 群聊 Session Key：`im:{provider_id}:group:{chat_id}`。
- 同一个飞书群加入多个机器人时，每个机器人对应独立 Provider，因此即使 `chat_id` 相同，也由 `provider_id` 隔离 Session、上下文游标、Runner 和工作目录。
- 单聊 Session Key 保持原兼容格式 `{provider_id}:{user_id}`。
- 第一条群消息创建逻辑会话和消息账本，但不启动 Runner。
- 每个模型触发区间为 `(last_assigned_seq, trigger_seq]`：排除上一次已分配的触发消息，包含本次触发消息。
- 普通管理 slash（例如 `/help`、`/status`、`/cwd`）不消耗增量上下文；`/clear` 和 `/reset` 同时建立新的上下文基线。
- `/g`、`/q` 和未知 slash 保持单聊执行语义，并携带本次增量群上下文。

## 数据路径

群消息账本使用 `admin/im_group_context.db`。SQLite 负责消息幂等、单调序号、游标和 Turn 的原子提交；`im_gateway_events.json` 继续只承担最近事件调试记录。

核心表：

- `im_group_messages`：按 `message_id` 去重保存群消息、发送者、mentions、回复关系、原始内容和接收来源；
- `im_group_bindings`：保存 session key、飞书群名称、群级工作目录、群级 Runner 和 `last_assigned_seq` / `last_success_seq`；
- `im_group_turns`：保存触发消息、增量区间、结构化输入、状态和错误。

## 触发状态机

1. 飞书事件先写入群消息账本。
2. 普通消息直接结束，不添加 reaction、不匹配 Agent route、不调用 Runner。
3. 触发消息先识别并移除当前机器人的 mention 占位符。
4. 系统 slash 原样交给现有单聊命令处理器。
5. Agent 触发在 SQLite 事务内冻结消息区间、生成 Turn 并推进 `last_assigned_seq`。
6. Session 空闲时启动 Turn；Session 运行中时复用现有 live Guide，失败时复用现有 queue fallback。
7. 直接启动 Runner 的 Turn 在完成后更新状态和 `last_success_seq`；运行中 Guide/Queue 的 Turn 保持 `dispatched`，其消息区间已由 `last_assigned_seq` 消费，冻结输入仍可审计。

## 模型输入

SQLite Turn 仍保存完整的消息 ID、序号、时间、mentions、游标和哈希供幂等与审计使用，但这些工程字段不注入模型。模型只接收紧凑的人类可读对话：

- 当前群第一次真正启动模型 Session 时，开头提供解析后的群名称和群 ID；后续轮次不重复群基本信息；
- 如果从上次执行到本次触发之间存在有效聊天，按原顺序逐行提供 `<at id=发送者 open_id>显示名</at>：消息内容`，并用一句固定边界说明这些内容只作背景；
- 已经由 Bifrost 处理过的 `/status`、`/help`、`/cwd`、`/runner` 等管理命令不进入模型背景；
- 如果没有收到有效的累积消息，完全省略背景区域；这既可能代表群内没有新聊天，也可能代表应用没有 `im:message.group_msg` 权限、飞书未投递普通群消息；
- 最后一条触发消息只出现一次，格式同样是 `<at id=发送者 open_id>显示名</at>：消息内容`。

`<at>` 是模型后续准确 @ 原发送人的稳定标识。飞书消息事件不保证携带发送者显示名；有名称时使用 `<at id=open_id>名称</at>`，没有名称时使用飞书最简形式 `<at id=open_id></at>`，不把冗长 open_id 复制到可见文本中。标签的 `id` 始终保持发送者 open_id；id 和显示名在序列化前进行转义，避免破坏结构或注入伪造标签。

## slash 一致性

群聊不新增 `/bind` 等别名。路径仍使用 `/cwd <绝对路径>`，Runner、模型、推理强度、队列、Guide、停止和重置均使用现有单聊命令：

`/help`、`/status`、`/cwd`、`/runner`、`/models`、`/model`、`/efforts`、`/effort`、`/clear`、`/reset`、`/q`、`/rq`、`/stop`、`/g`。

群聊 `/cwd` 和 `/runner` 只更新当前群 binding；单聊仍保持 provider 级兼容行为。模型与推理强度继续使用已有的 session state，并因群 Session Key 天然隔离。

分类边界复用单聊解析器的精确语义：例如 `/help` 是系统命令，`/help extra` 会像单聊一样回落到模型；`/rq <序号>` 只在 Session 忙碌时作为队列管理命令，空闲时按单聊行为进入模型。群聊仅在回落到模型时增加结构化多人上下文，不改变命令本身的可用时机和响应。

## 飞书能力与权限前提

- 订阅事件：`im.message.receive_v1`，事件体提供 `chat_type`、`chat_id`、`message_id`、发送者、消息内容、mentions 和回复/话题关系，可区分单聊与群聊。
- 只接收 @ 消息可使用 `im:message.group_at_msg:readonly`；要让未 @ 的普通群聊也进入账本，必须申请敏感权限 `im:message.group_msg`，并在权限变更后重新发布应用版本。
- 回复群消息需要 `im:message:send_as_bot`（或包含发送能力的 `im:message`）；读取消息资源需要 `im:resource`。
- `GET /open-apis/bot/v3/info` 用于解析当前机器人 `open_id`，从而区分“@机器人”和“@其他成员”。
- @ 判定优先严格比较 mention `open_id` 与当前 Provider 的 bot `open_id`；即使两个机器人重名，也不能用名称覆盖不匹配的 `open_id`，避免同群多机器人串触发。
- 接收事件中的 `sender_id.open_id` 是发送者身份主键；每条历史消息和主动请求都以飞书 `<at id=...>...</at>` 结构带入 Prompt。显示名不可用时使用空标签正文 `<at id=open_id></at>`，不因额外通讯录权限缺失阻断群 Session。
- 消息事件只携带 `chat_id`；首次初始化群 Session 时调用 `GET /open-apis/im/v1/chats/{chat_id}` 解析群名称，写入 binding 并随每轮结构化输入提供给 Agent。应用需具备“获取群组信息”（如 `im:chat:readonly`）能力；机器人必须在目标群内。
- 飞书提供 `GET /open-apis/im/v1/messages` 获取会话历史消息；如启用断线补偿，还需要 `im:message:readonly`，且只能读取应用有权访问的会话范围。

默认模型输入仍只使用 Bifrost 本地增量账本。Bifrost 只在群 Session 首次启动对话的用户 Prompt 中提供群名称和群 ID；不注入 provider、Session Key、消息 ID、游标等内部字段，也不在业务 Prompt 中指定 Agent 应使用何种工具。运行环境安装的工具能力及规则由对应 Skill 独立注入 Context。

## 卡片响应样式

- Bifrost 生成的飞书交互卡片统一不使用根级 `header`，避免占据大块空间的彩色标题栏；任务计划、工具记录、执行过程等卡片内部的折叠面板标题继续保留。
- 由用户消息触发的普通回复卡片和 CardKit 进度卡片使用 `POST /open-apis/im/v1/messages/{message_id}/reply` 发送。飞书客户端以原生轻量引用展示触发消息，不在卡片正文中重复人工引用文本。
- 同一 Session 后续 Turn 的新进度卡片回复各自最新的触发消息；卡片大小回退产生的续卡仍引用当前 Turn 的触发消息。
- 上线通知、定时任务、管理端主动发送等没有来源消息的卡片继续按目标会话发送，但同样移除根级标题。
- Feishu Provider 在普通发送、消息 PATCH、CardKit 创建和 CardKit 更新边界统一移除根级 `header`，作为全链路样式兜底。

## 权限与安全

- 全部群成员的消息都可进入账本，不能再在接收入口按 owner 丢弃。
- 普通成员可以通过 @ 或执行类 slash 触发当前群 Session。
- provider owner 仍是管理和在线通知身份；后续可在 binding 上扩展管理员 allowlist。
- 群上下文按不可信数据处理，只有 `active_request` 具有指令语义。
- 群 Runner 的工作目录来自群 binding，避免一个群的 `/cwd` 改变其他群。
- 普通群消息不添加 `OK` reaction，只有实际触发消息沿用确认行为。

## 容量和恢复

- 已接收的原始消息全量保存在账本中，结构化输入不得静默裁剪；输入超过 200 条或 64 KiB 时返回可观察错误且不推进游标，后续可扩展摘要/附件能力。
- 首版以长连接持续订阅为主，不主动调用历史消息 API。飞书具备历史读取能力，但自动断线补偿会额外扩大敏感权限和数据读取范围，应作为显式开关实现；当前断线窗口是已知边界。
- Turn 创建和游标推进必须在同一事务中完成，避免崩溃后重复或跳过区间。
- 一个 Runner 活跃期间收到的其他群消息先写入账本，再进入跨 Session 待处理队列，避免长任务期间进程异常造成已接收群消息丢失。
- 同一群内顺序执行；不同群拥有独立 Session Key，不共享对话历史、队列或工作目录。

## 验证清单

- 普通群消息只入账且不运行模型；
- 首次 @ 包含绑定启用后的全部普通消息；
- 第二次 @ 只包含上次触发后的增量；
- 运行中 @ 和 `/g` 携带新增上下文进入 Guide；
- 两个群的 Session、游标和 `/cwd` 互不影响；
- slash 解析及响应与单聊一致；
- 重复事件不会产生重复消息或重复 Turn；
- 非 owner 群成员消息能入账并可通过 @ 触发；
- 历史消息和主动请求都包含发送者 `sender_at`，且恶意名称不能逃逸标签结构；
- 普通消息不产生 reaction；
- 首轮 Prompt 只额外包含群名称和群 ID，后续 Prompt 不重复基本信息；
- 无背景消息时 Prompt 只有最后一条带发送者 `<at>` 的当前消息；
- 完整审计字段不泄露到模型 Prompt。
