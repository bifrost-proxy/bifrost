# 飞书群话题 Agent 派生

## 目标与边界

本能力只处理飞书群聊中的话题消息。话题使用独立的 Bifrost session；群主会话继续使用
`im:{provider}:group:{chat}`，话题使用
`im:{provider}:group:{chat}:thread:{thread_id}`。私聊行为不变。

话题首条回复按本机、当前 provider 的持久化锚点认领：

1. 根消息命中本地 Agent 卡片锚点时，从锚点对应的 Runner checkpoint 派生。
2. 根消息未命中本地锚点时（含另一设备上的 Bot 输出），按普通消息派生；输入只包含根消息和当前消息，不读取或累计群历史。
3. 普通消息只有显式 `@当前 Bot` 才认领。未提及当前 Bot、只提及其他 Bot，均忽略。
4. `(provider_id, chat_id, thread_id)` 唯一约束保证重复事件只初始化一次。

本版本不在设备间复制 Agent 历史，也不为“同一飞书应用同时运行在两台设备”提供分布式租约。该部署方式可能由两台设备同时认领，需由部署侧避免。

## 数据模型

在 `admin/im_group_context.db` 增加两张表：

- `im_feishu_message_anchors`：把本机发出的卡片 `message_id` 绑定到 provider、群、Runner、外部 thread/turn/checkpoint 和终态。
- `im_feishu_thread_bindings`：把飞书 `thread_id` 绑定到独立 Bifrost session，记录来源是本地 checkpoint 或普通消息，并保存初始化/等待/就绪/失败状态。

锚点按 `(provider_id, message_id)` 唯一；话题绑定按
`(provider_id, chat_id, feishu_thread_id)` 唯一。所有初始化操作使用 SQLite 事务和 upsert，事件重投不会创建第二个 Agent session。

话题消息仍写入群消息账本，便于审计和根消息读取，但主群增量上下文查询明确排除 `thread_id IS NOT NULL` 的记录，话题不推进主群上下文游标。

## Runner 能力模型

Runner 以能力而非名称分支：

| Adapter/transport | 完成后 fork | 运行中 fork | 精确 turn fork | v1 行为 |
| --- | --- | --- | --- | --- |
| Codex app-server | 是 | 是 | 是 | `thread/fork`，传 `threadId` 和 `lastTurnId` |
| Traex app-server | 是 | 否 | 否 | 终态建立 checkpoint fork；运行中请求持久化等待 |
| exec/custom | 否 | 否 | 否 | 普通双消息 fresh start，不把 resume 冒充 fork |
| Claude Code | 未知 | 未知 | 未知 | 保留 adapter capability 扩展点；当前返回 unavailable |

Codex 锚点保存该卡片所属 `turnId`，因此后续即使源 thread 又执行过多轮，仍能精确从旧卡片派生。运行中收到请求时，Codex 可直接 fork 已完成的 `lastTurnId`；若锚点尚无可安全复制的 turn，则进入等待状态，避免复制半轮状态。

Traex 的 `thread/fork` 没有 `lastTurnId`。每轮终态、源会话开始下一轮之前建立不可变 checkpoint，并把该轮所有卡片绑定到 checkpoint。运行中话题请求写为 `waiting_source`，在源轮终态创建 checkpoint 后优先完成派生，再继续源会话队列。

## 飞书消息处理

事件必须同时满足：provider 为飞书、`chat_type=group`、消息具有非空 `thread_id` 和根消息 ID。根 ID 优先取 `root_id`，缺失时取 `parent_id`。

处理顺序：

1. 查询现有话题绑定；存在则路由到绑定 session。
2. 查询本地消息锚点；ready 时按 Runner 能力 fork，pending 时按能力直接 fork 或等待源终态。
3. 没有本地锚点时通过飞书消息读取 API 获取根消息，并验证 `chat_id`，然后构造严格的“根消息 + 当前消息”提示。
4. 创建独立话题 session，后续同一话题消息只进入该 session，不回到主群 session。

输出卡片在话题内调用飞书 reply API 并设置 `reply_in_thread=true`。已有话题下的回复保持在该话题；普通群消息行为不变。

## 失败与恢复

- 飞书根消息无法读取或跨群：拒绝派生，不泄露其他群内容。
- 本地锚点已失败：降级为普通双消息 fresh start，并记录降级原因。
- 等待中的源运行失败：将话题 binding 标记失败并给用户明确反馈；不复制部分历史。
- 进程重启：SQLite 中的 binding/anchor/pending 状态恢复；初始化状态可幂等重试。
- Claude Code：不声称已验证，能力查询为 unavailable；未来只需实现 capability 与 fork primitive，不改飞书路由。

## 验证

- 单元测试覆盖 session key、话题识别、当前 Bot 认领、严格双消息提示、幂等 binding/anchor、主群排除话题消息、能力矩阵和 fork JSON-RPC。
- E2E 使用 fake Feishu OpenAPI 与 mock Codex/Traex app-server，覆盖本地锚点、缺锚点（跨设备等价）、多 Bot 隔离、运行中等待和 `reply_in_thread`。
- `human_tests/feishu-thread-agent-derivation.md` 覆盖真实群聊感知行为；Claude Code 标明环境不可用，不伪造运行验证。
