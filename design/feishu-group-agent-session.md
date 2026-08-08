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
- 单聊 Session Key 保持原兼容格式 `{provider_id}:{user_id}`，同一 Provider 下不同私聊用户也各自隔离。
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
   - 如果触发消息带有 `parent_id`，则该父消息是本轮主要处理对象；即使移除机器人 mention 后没有附加文字，也必须继续创建 Agent Turn，不能回落到 `/help`。
4. 系统 slash 原样交给现有单聊命令处理器。
5. Agent 触发在 SQLite 事务内冻结消息区间、生成 Turn 并推进 `last_assigned_seq`。
6. Session 空闲时启动 Turn；Session 运行中时复用现有 live Guide，失败时复用现有 queue fallback。
7. 直接启动 Runner 的 Turn 在完成后更新状态和 `last_success_seq`；运行中 Guide/Queue 的 Turn 保持 `dispatched`。live Guide 被外部 Runner 接受后也必须等当前 Runner 成功或失败再同步完成或失败，不能在控制通道仅返回 `accepted` 时提前记为成功。未显式绑定 Runner 时仍按实际默认 Runner adapter 选择 live Guide 或 Queue；若 Guide 不能在当前进程内被消费而转为延后执行，队列项必须保留原 `group_turn_id`，由延后 Turn 的最终结果收尾。
8. 进程重启会丢失内存队列和事件去重缓存，但 SQLite 中 `prepared` / `dispatched` Turn 保留冻结输入。飞书重投同一触发消息且对应 Session 当前空闲时，复用原 Turn 继续执行；同进程 Session 仍繁忙时继续按重复消息丢弃，避免双跑。

## 并发分发与进程隔离

Provider 长连接只负责接收事件，不把全局接收器借给任何一个 Runner。事件进入后先计算统一 Session Key，再由 Provider 分发器直接投递到对应 mailbox：群聊按 `(provider_id, chat_id)`，私聊按 `(provider_id, user_id)`。

- 每个活跃 Session 拥有独立 mailbox、异步控制任务和外部 Runner 进程；群 A、群 B、私聊 A、私聊 B 不共享接收器，也不互相等待。
- 同一 Session 的后续消息仍按到达顺序进入同一 mailbox，并复用 Guide、Queue、`/stop` 和进度控制语义。
- Provider 主循环始终继续接收新事件；启动 Runner 只注册 mailbox 并派生 Session 任务，不在主循环内等待 Runner 完成。
- Session 任务完成通知携带单调 generation。旧任务延迟完成时只能移除同 generation 的 mailbox，不能误删已经重建的新任务。
- Provider 输入通道正常关闭时停止接收新消息，但等待已启动的 Session 任务完成并排空收尾消息；event-loop 被显式取消时才终止仍活跃的任务。异常取消或 panic 会释放 Session active 标记，避免永久 busy。
- Runner 完成最后一次 Guide/Queue 检查、确认没有下一条消息后，必须立即关闭 mailbox receiver，再执行进度卡、历史和 Session 的异步收尾。关闭前已经送达但尚未消费的事件由 task wrapper 按顺序 drain；关闭后 sender 失败的事件由 registry 暂存，两类事件都在 completion 时交回 Provider 主循环。
- mailbox receiver 关闭后的消息暂存在同 generation 的收尾队列；completion 先回放 receiver 中更早的缓冲消息，再回放关闭后消息并创建替代任务，不能因完成通知竞态而丢失或打乱同 Session 顺序。
- 并发 E2E 使用显式 release 文件保持 mock Runner 活跃，并在释放前等待同 Session 的最后一条背景消息进入 SQLite 账本。测试不得依赖固定 sleep 恰好覆盖 CI 调度延迟，否则会把 Runner 已结束后的合法空闲态行为误报为活跃态审计回归。
- 飞书假 OpenAPI 只允许 debug 构建在显式 `BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1` 时访问。CI shell 矩阵复用的 production release 必须跳过该假服务场景，不能为了黑盒测试放宽生产 host allowlist，也不能把测试凭证发到官方接口。E2E pipeline contract 会扫描全部使用该开关的 shell 脚本，强制它们声明 release guard 和统一的 skip 标记。完整群会话链路由 focused debug E2E 覆盖；release-safe 单元测试继续覆盖 URL 规范化、引用卡片读取、CardKit 回退和权限错误矩阵。

## 模型输入

SQLite Turn 仍保存完整的消息 ID、序号、时间、mentions、游标和哈希供幂等与审计使用，但这些工程字段不注入模型。模型只接收紧凑的人类可读对话：

- 当前群第一次真正启动模型 Session 时，开头提供解析后的群名称和群 ID；后续轮次不重复群基本信息；
- 如果从上次执行到本次触发之间存在有效聊天，按原顺序逐行提供 `<at id=发送者 open_id>显示名</at>：消息内容`，并用一句固定边界说明这些内容只作背景；
- 已经由 Bifrost 处理过的 `/status`、`/help`、`/cwd`、`/runner` 等管理命令不进入模型背景；
- 如果没有收到有效的累积消息，完全省略背景区域；这既可能代表群内没有新聊天，也可能代表应用没有 `im:message.group_msg` 权限、飞书未投递普通群消息；
- 最后一条触发消息只出现一次，格式同样是 `<at id=发送者 open_id>显示名</at>：消息内容`。
- 回复消息带有 `parent_id` 且当前 Provider 确认该消息应由自己消费时，通过飞书 `GET /im/v1/messages/{parent_id}` 读取权威消息内容，再以当前 Provider 的本地账本作为 Prompt 组装缓存；不同机器人可以部署在不同机器上，不依赖共享进程内存、本地 JSON 或共享 SQLite。返回的 `chat_id` 必须与当前群一致，跨群引用直接拒绝。若它同时位于当前增量区间，则从普通背景中排除，避免重复注入。
- `回复某条消息 + 只 @机器人` 是有效 Agent 输入。此时被引用消息本身作为主消息，并明确告诉模型用户未附加文字、应直接理解和回应主消息；不能因去除 mention 后文本为空而发送帮助信息。
- 飞书读取失败时不得让模型猜测。权限不足时直接告知用户在开放平台申请 `im:message:readonly` 与 `im:message.group_msg`，然后创建并发布新版本；网络或 API 错误也按真实原因返回。
- 交互卡片会提取标题、Markdown、正文和折叠区等可见文本，忽略按钮、URL 和动作 value。CardKit 首次只返回 `card_id` 时再读取默认消息表示，获得用户当前可见内容。历史图片、文件或音视频不会自动补拉二进制资源。

`<at>` 是模型后续准确 @ 原发送人的稳定标识。飞书消息事件不保证携带发送者显示名；有名称时使用 `<at id=open_id>名称</at>`，没有名称时使用飞书最简形式 `<at id=open_id></at>`，不把冗长 open_id 复制到可见文本中。标签的 `id` 始终保持发送者 open_id；id 和显示名在序列化前进行转义，避免破坏结构或注入伪造标签。

飞书富文本 `post` 的 `at` 节点只携带用户 ID/显示名，正文里不一定包含事件 `mentions[].key`。归一化阶段必须先按用户 ID、再按显示名把 `at` 节点恢复成稳定 placeholder，之后再统一执行 bot mention 移除和 `<at>` 渲染；不能把易变显示名直接送入群 Prompt。

## slash 一致性

群聊不新增 `/bind` 等别名。路径仍使用 `/cwd <绝对路径>`，Runner、模型、推理强度、队列、Guide、停止和重置均使用现有单聊命令：

`/help`、`/status`、`/pwd`、`/cwd`、`/runner`、`/new`、`/models`、`/model`、`/efforts`、`/effort`、`/clear`、`/reset`、`/q`、`/rq`、`/stop`、`/g`。其中 `/q` 无参数列出当前线程队列，`/pwd` 输出当前线程工作目录，`/runner` 无参数输出当前有效 Runner。

群里没有 `@` 的 slash 命令广播给所有机器人；一旦出现明确 `@`，只有被提及机器人才消费。这个判断先于命令分类和引用读取，因此未被提及的 Provider 不读取引用、不修改本地状态，也不执行命令。

群聊 `/cwd` 和 `/runner` 只更新当前群 binding；单聊仍保持 provider 级兼容行为。模型与推理强度继续使用已有的 session state，并因群 Session Key 天然隔离。

Runner 回复中的相对图片和文件链接同样以当前群 binding 的工作目录为基准解析；只有群没有绑定工作目录时才回退到 Provider 默认目录，避免从其他项目读取或上传同名资产。

分类边界复用单聊解析器的精确语义：例如 `/help` 是系统命令，`/help extra` 会像单聊一样回落到模型；`/rq <序号>` 只在 Session 忙碌时作为队列管理命令，空闲时按单聊行为进入模型。群聊仅在回落到模型时增加结构化多人上下文，不改变命令本身的可用时机和响应。

### `/new <群名>` 创建飞书群

- `/new <群名>` 是 Feishu Provider 级快速命令，不依赖 Agent 是否启用，也不进入 Runner、Route、Guide 或 Queue。即使当前单聊或群聊 Session 正忙，仍立即执行建群请求。
- 命令只接受精确的 `/new` token；`/new-project`、`/newer` 等继续按普通消息处理。`/new` 或仅空白参数返回用法，群名按 Unicode 字符计数不得超过 60 个字符。
- 命令在单聊和群聊都只允许当前 Provider 的 `owner_open_id` 执行。群聊仍允许普通成员触发 Agent，但建群属于外部写操作，必须单独 fail-closed；Provider 未配置 owner、事件缺少发送者 open_id 或发送者不匹配时拒绝创建。
- 创建请求固定为普通私有群：`chat_mode=group`、`chat_type=private`；命令发送者的 `open_id` 写入 `owner_id`，因此发送者作为群主和成员进入新群。调用接口的当前应用机器人由飞书自动加入，并通过 `set_bot_manager=true` 设为群管理员，无需把自身 App ID 重复写入 `bot_id_list`。
- 以 `provider_id + source message_id` 生成稳定的 Feishu `uuid`。飞书在 `uuid + owner_id` 维度提供 10 小时创建去重；Bifrost 同时把成功结果持久化到本地命令账本，重投或重启后重放同一消息只返回原 `chat_id`，不再次创建群。
- 创建成功后先向新群发送一条欢迎消息，确认机器人已入群且可发送；随后在原会话回复群名和 `chat_id`。欢迎消息失败不回滚已经创建的群，但原会话必须明确提示该部分失败。
- 飞书应用必须开通机器人身份的 `im:chat:create`（或包含该能力的 `im:chat`）以及 `im:message:send_as_bot`，权限变更后重新发布应用版本。权限不足、用户不可见、租户边界或网络错误必须原样归因为可行动错误，不能回落到模型猜测。

## 飞书能力与权限前提

- 订阅事件：`im.message.receive_v1`，事件体提供 `chat_type`、`chat_id`、`message_id`、发送者、消息内容、mentions 和回复/话题关系，可区分单聊与群聊。
- 只接收 @ 消息可使用 `im:message.group_at_msg:readonly`；要让未 @ 的普通群聊也进入账本，必须申请敏感权限 `im:message.group_msg`，并在权限变更后重新发布应用版本。
- 回复群消息需要 `im:message:send_as_bot`（或包含发送能力的 `im:message`）；读取消息资源需要 `im:resource`。
- `GET /open-apis/bot/v3/info` 用于解析当前机器人 `open_id`，从而区分“@机器人”和“@其他成员”。
- @ 判定优先严格比较 mention `open_id` 与当前 Provider 的 bot `open_id`；即使两个机器人重名，也不能用名称覆盖不匹配的 `open_id`，避免同群多机器人串触发。
- 接收事件中的 `sender_id.open_id` 是发送者身份主键；每条历史消息和主动请求都以飞书 `<at id=...>...</at>` 结构带入 Prompt。显示名不可用时使用空标签正文 `<at id=open_id></at>`，不因额外通讯录权限缺失阻断群 Session。
- 消息事件只携带 `chat_id`；首次初始化群 Session 时调用 `GET /open-apis/im/v1/chats/{chat_id}` 解析群名称，写入 binding 并随每轮结构化输入提供给 Agent。应用需具备“获取群组信息”（如 `im:chat:readonly`）能力；机器人必须在目标群内。
- 飞书提供 `GET /open-apis/im/v1/messages` 获取会话历史消息；如启用断线补偿，还需要 `im:message:readonly`，且只能读取应用有权访问的会话范围。

默认增量群背景仍使用每个 Provider 自己的本地账本；引用消息则以飞书消息读取 API 为权威来源，读取后只缓存到本 Provider 的账本做本轮 Prompt 组装。Bifrost 只在群 Session 首次启动对话的用户 Prompt 中提供群名称和群 ID；不注入 provider、Session Key、消息 ID、游标等内部字段，也不在业务 Prompt 中指定 Agent 应使用何种工具。运行环境安装的工具能力及规则由对应 Skill 独立注入 Context。

## 卡片响应样式

- 非目标：发送回复后自动打开飞书客户端的悬浮窗、话题窗或侧边回复面板。Bifrost 只建立原生消息回复关系，不控制客户端如何展开或展示该关系。
- Bifrost 生成的飞书交互卡片统一不使用根级 `header`，避免占据大块空间的彩色标题栏；任务计划、工具记录、执行过程等卡片内部的折叠面板标题继续保留。
- 由用户消息触发的普通回复卡片和 CardKit 进度卡片使用 `POST /open-apis/im/v1/messages/{message_id}/reply` 发送。飞书客户端以原生轻量引用展示触发消息；点击引用条可定位并打开原始消息，不在卡片正文中重复人工引用文本或伪造跳转链接。
- 同一 Session 后续 Turn 的新进度卡片回复各自最新的触发消息；卡片大小回退产生的续卡仍引用当前 Turn 的触发消息。
- 上线通知、定时任务、管理端主动发送等没有来源消息的卡片继续按目标会话发送，但同样移除根级标题。
- Feishu Provider 在普通发送、消息 PATCH、CardKit 创建和 CardKit 更新边界统一移除根级 `header`，作为全链路样式兜底。

## 非目标：机器人自定义菜单

Bifrost 不创建、更新或同步飞书机器人自定义菜单，也不订阅或解释机器人菜单事件。群会话和单聊控制统一继续使用 `@机器人` 与现有 slash 命令，避免把消息链路绑定到需要额外权限、后台发布配置和客户端展示策略的菜单能力。

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
- 一个 Runner 活跃期间收到的其他群或私聊消息直接投递到各自 Session mailbox，并立即启动各自外部进程，不进入跨 Session 待处理队列。
- 同一群或同一私聊内顺序执行；不同群、不同私聊以及群聊与私聊之间拥有独立 Session Key，不共享进程、对话历史、队列或工作目录。
- Provider 入口在查找 Session mailbox 之前按 `message_id`（缺失时回退 `event_id`）去重；成功投递到 live mailbox 后才记录 dedup。receiver 已关闭但 completion 未处理的消息先缓冲回放，避免过早记录导致回放被误删。
- 活跃 Session 的消息在 Session 任务内完成触发分类：被接受的群触发和私聊消息保留 `OK` reaction 与 inbound audit，普通群背景消息只进入上下文账本，不产生触发确认副作用。

## 验证清单

- 普通群消息只入账且不运行模型；
- 首次 @ 包含绑定启用后的全部普通消息；
- 第二次 @ 只包含上次触发后的增量；
- 运行中 @ 和 `/g` 携带新增上下文进入 Guide；
- 两个群的 Session、游标和 `/cwd` 互不影响；
- 两个群与两个私聊可同时保持 Runner active，任一长任务不阻塞其他会话接收、启动或回复；
- slash 解析及响应与单聊一致；
- 重复事件不会产生重复消息或重复 Turn；
- 非 owner 群成员消息能入账并可通过 @ 触发；
- 历史消息和主动请求都包含发送者 `sender_at`，且恶意名称不能逃逸标签结构；
- 普通消息不产生 reaction；
- 活跃 Session 的重复触发只执行、确认和审计一次，mailbox 关闭竞态中的回放消息不丢失；
- 首轮 Prompt 只额外包含群名称和群 ID，后续 Prompt 不重复基本信息；
- 无背景消息时 Prompt 只有最后一条带发送者 `<at>` 的当前消息；
- 引用并 @ 机器人时，被引用消息作为主输入且只出现一次；只 @、无附加文字也会运行 Agent，不返回 `/help`；
- 引用早于游标的同群消息仍能读取，引用不存在或指向其他群时不会跨群泄露；
- 完整审计字段不泄露到模型 Prompt。
