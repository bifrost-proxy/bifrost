# Agent Run Timeline 与多通道展示统一方案

## 背景

当前 Agent 可以从多个入口触发：

- Web Agent Chat：`/api/agent/chat/stream` 或 `/api/im-gateway/agent/chat`
- IM Gateway：飞书、微信等入站消息触发内置 Bifrost Agent
- External Runner：Codex、ChatGPT Web、自定义 CLI Runner
- Schedule / Manual Run：定时任务或手动触发 Agent

产品预期是：这些入口只是消息投递与结果展示通道不同，底层 Agent Loop、执行过程、工具调用、plan、compaction、token/context 状态应该落在同一个可回放的 timeline 上。用户从 IM 发起的任务，也应该能在 Web Chat 页面看到完整执行过程，而不是只看到用户消息和最终 AI 输出。

更进一步，Loop 应该下沉到 conversation/session 层，视图应该上提到 channel projection 层。Web UI、飞书、微信等通道都只是同一个 conversation 的输入端和展示端。只要它们绑定的是同一个对话：

- 从 IM 发出的用户消息，Web UI 应该实时看到。
- 从 Web UI 发出的用户消息，IM 通道也应该看到。
- Agent 的运行状态、工具调用、最终回复，不应该只更新触发通道，而应该 fan-out 到所有绑定通道。
- 对飞书这类卡片通道，Web 触发的 turn 也应该创建或更新对应 progress card，而不是只有 Web 面板刷新。
- Web、IM、不同 Agent/Runner 的运行状态也应该使用同一份底层状态，不应该在各自通道里拆出互不感知的 running/completed/failed 状态。
- 历史数据兼容不作为本方案目标；允许直接收敛到新的 canonical timeline + channel binding 数据模型，避免为了旧读写路径保留复杂分支。

## 现象

通过 IM 通道发起 Agent Loop 后，Web 页面打开同一个 chat/thread，通常只能看到：

- user message
- assistant final response

缺失内容包括：

- tool started / finished
- tool arguments / result preview
- plan updates
- compaction events
- context/token runtime changes
- long task status

这说明 Loop 过程数据没有被 Web Chat 的当前读取路径完整消费。

## 当前实现观察

### Web Stream 路径

`crates/bifrost-admin/src/handlers/agent_chat.rs` 的 Web stream 路径会给 session 设置 `progress_sender`，并把 `AgentTurnProgressEvent` 转成 SSE：

- `ToolStarted` -> `tool_started`
- `ToolFinished` -> `tool_finished`
- `PlanUpdated` -> `plan_updated`
- `Compaction*` -> `compaction_*`
- `AssistantDelta` / `AssistantFinal`

前端 `AgentChatSection` 运行中消费这些 SSE，所以 Web 自己发起的 turn 能看到执行过程。

### IM 内置 Agent 路径

`crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs::process_agent_chat()` 也会创建 `ConversationRecorder`，并调用同一个 `run_turn_with_mcp_multimodal()`。底层 turn loop 会写 JSONL events：

- `user_message`
- `tool_call`
- `tool_result`
- `assistant_message`
- `plan_updated`
- `compaction`
- runtime state events

同时 IM 路径还会把 `progress_sender` 连接到 `ImAgentProgressRegistry`，用于飞书 progress card 实时展示。

也就是说，底层并不是完全没有过程数据；问题主要出在 Web Chat 读取和会话索引模型。

### Web Chat 读取路径分裂

前端当前有两套读取路径：

1. active session detail
   - API：`GET /api/im-gateway/agent/sessions/:key`
   - 后端优先调用 `agent_session_manager.get_session_detail()`
   - 返回 `SessionDetail.messages`
   - 前端 `sessionDetailToMessages()` 只保留 user/assistant

2. history event detail
   - API：`GET /api/im-gateway/agent/sessions/history/:path`
   - 返回 JSONL `events`
   - 前端 `historyEventsToTelemetry()` 才会解析 `tool_call/tool_result/plan_updated/compaction`

关键问题：`/sessions/all` 构造列表时，如果同一个 `session_key` 已经存在 active/idle in-memory session，就跳过 history row。IM turn 完成后 session 会被 `return_session()` 放回内存，因此 Web 点击该 thread 时进入 active 视图，而不是 history event 视图。

结果就是：完整 JSONL timeline 存在，但被 active session detail 遮蔽，Web Chat 看不到过程事件。

### External Runner 路径的额外问题

External Runner/Codex/ChatGPT Web 还有一层 `session_state.json`。当前 `record_external_cli_result()` 会把 `ExternalCliRunResult.events` 和 artifacts 塞进一个 `tool_result` JSON blob，并只额外保存 `event_types`。这对审计有用，但还不是 Web Chat 可以直接消费的标准 timeline。

因此 External Runner 的过程展示需要两步：

- 保留 run 级 JSONL/persisted artifacts
- 把 runner 私有 progress / normalized events 归一化成标准 `ConversationEvent`

Web 触发 External Runner 时还存在一个更隐蔽的分叉：如果当前页面已经绑定了内置 Agent/IM 产生的 `history_path`，Web 请求只更新 `session_state.messages`，但没有把本轮 user/assistant 写回同一个 JSONL。刷新后 history view 会重新读取旧 JSONL，并覆盖掉只存在于 `session_state` 的 Web 消息，表现为用户消息消失、running/thinking 挂到上一轮 assistant 下方。

因此 `session_state.json` 只能作为线程索引、外部 thread/conversation 引用、latest run id 和运行摘要缓存；它不能成为消息事实源。External Runner 每一轮，无论来自 Web 还是 IM，都必须追加到 canonical timeline：

- `run_state_changed` 标记 `source_channel=web|im` 与 `agent_kind=<runner_id>`
- `user_message`
- 外部 runner 作为标准 `tool_call/tool_result`
- `assistant_message`

当 Web 请求携带已有 `historyPath` 时，后端必须校验并追加到该 JSONL；当没有历史文件时，后端创建新的 timeline，并把路径回写到 `session_state.history_path` 供线程列表和刷新恢复使用。

## 根因结论

当前系统把“通道视图”误当成了“会话数据边界”：

- IM progress card 是一套实时展示面。
- Web SSE 是另一套实时展示面。
- JSONL history 是持久化事件面。
- active `SessionDetail.messages` 是精简聊天消息面。

这些面没有统一成一个 canonical run timeline。Web Chat 在 active session 存在时优先读精简消息面，于是丢失了 Loop 内部过程。

更深层的问题是：当前运行时仍然隐含了“触发通道 = 唯一展示通道”的假设。`progress_sender` 通常只挂到当前请求路径创建的 sink：

- Web 发起时，sink 是 Web SSE。
- IM 发起时，sink 是 IM progress registry/card。

这会导致同一个 conversation 上的跨通道输入无法自然同步。例如飞书用户先在 IM 发起一个 Agent task，随后在 Web UI 打开同一个 thread 并继续发送消息。此时底层 session 应该仍是同一个 Loop，但回复和状态更新需要同时回到 Web thread 与飞书卡片；如果 sender 仍是单一通道 sink，就会出现“Web 看到回复，飞书卡片不更新”或反过来的问题。

同样的问题也存在于运行状态层：如果 Web、IM、External Runner、内置 Agent 各自维护状态，就会出现 Web 认为 idle、IM card 仍显示 running，或者 runner 子任务失败但父 conversation 视图无感知。运行状态必须从 Loop/AgentRun 层统一产生，再投影到不同 channel view。

## 设计目标

1. 多通道只影响输入输出，不影响底层 Loop timeline。
2. 同一 `session_key` 的同一 turn 只写一条 canonical timeline。
3. Web、IM、API、Schedule 都能从同一 timeline 投影出各自展示。
4. Web Chat 刷新后仍能看到历史工具调用、plan、compaction 和最终输出。
5. running 状态与 persisted timeline 能合并展示：运行中看实时事件，结束后看可回放事件。
6. External Runner 的私有事件必须先归一化，再进入 Web/IM 通用展示。
7. 同一 conversation 绑定多个 channel 时，任意 channel 发起的消息和 Agent 回复都要广播到所有绑定 channel。
8. 通道只保存展示状态与投递游标，不拥有 Loop 状态；Loop 不应该依赖“当前触发通道”决定写哪份过程数据。
9. 不同 Agent/Runner 的状态统一进入 `AgentRunState` / canonical timeline，所有视图从同一状态机感知 queued/running/waiting/failed/completed。
10. 已结束的 Web Chat turn loop 默认折叠过程消息，只保留 `已处理 <duration>` 摘要与最终 assistant 结论；展开后仍能查看完整 assistant delta、工具调用、plan、compaction 等过程。
11. 不做旧数据兼容设计；相关 API、前端 helper 和持久化读写可以直接按新模型收敛。

### 运行态一致性真源

`session_state.json` 只能作为线程索引、外部 runner 引用、latest run id 和展示摘要缓存，不能作为 `running=true` 的唯一事实源。运行态投影必须按以下优先级收敛：

1. 当前进程的 live runtime registry（内置 Agent active turn、worker stop registry、external CLI worker registry）是“仍在运行”的唯一实时证明。
2. canonical JSONL timeline 的最新终态 `run_state_changed` 是“已经结束”的持久化事实源，终态包括 `completed`、`failed`、`stopped`、`timed_out`。
3. `/sessions/all` 合并 persisted `session_state` 时，如果同一 `session_key` 已有 JSONL 终态，必须按 JSONL 终态投影，并使用 JSONL 终态时间作为排序时间，避免陈旧 `updatedAt` 把 ended history 顶到 running。
4. 内置 `bifrost_agent` 的 persisted `status:"running"` 如果没有当前进程 live runtime 证明，应视为陈旧残留并投影为 ended/completed；内置 Agent 没有跨装置持久运行语义。
5. 所有非内置 runner adapter（例如 `codex`、`chatgpt_web`、自定义 CLI adapter）在没有 JSONL 终态且没有本地 live registry 证明时，仍可保留 persisted `running`，用于兼容不同装置或外部 runner 的异步状态。
6. `/stop` 先请求 live runtime 停止；如果没有命中任何正在运行的 loop，但 persisted state 确认为陈旧 running，应同步修复 `session_state.json` 为 terminal status，并在响应中暴露修复计数。

测试方案：

- 单元测试覆盖内置 Agent 陈旧 running、JSONL 终态覆盖、所有非内置 runner adapter 无终态时保持 running、陈旧状态修复落盘。
- E2E 使用临时 `BIFROST_DATA_DIR` 构造 completed JSONL + stale `bifrost_agent` running state，并同时构造 `codex`、`chatgpt_web`、`custom_cli` running state，验证 `/sessions/all`、`/stop` 和落盘状态一致。
- human_tests 在 `human_tests/agent-session-persistence.md` 记录陈旧 Running 状态与 Stop 一致性回归，并真实执行对应 E2E。
- Review/Fix/Test 至少两轮复核状态投影、runner 兼容、history 排序、stop 响应和文档/测试一致性。

### 完成态 Loop 折叠展示

Web Agent Chat 的消息区按 `user_message` 切分 turn：一个用户输入及其后的连续 assistant 片段属于同一个 loop 展示组。当前仍在运行的最后一个 turn 保持展开，继续实时显示 delta、process block、Thinking tail 和工具状态；已经结束的 turn 默认收起中间过程，只在用户消息下方显示轻量摘要行：

```text
已处理 4m 33s >
```

摘要耗时来自该 turn 的 user 消息时间戳到最终 assistant 输出时间戳的差值；没有可靠时间戳时显示 `<1s`。默认收起状态下只渲染最后一个可见 assistant 输出（文本、图片或 runner call），并隐藏其 process steps，避免工具调用和状态日志把历史页面撑长。点击摘要行后，按原始顺序恢复渲染该 turn 内的所有 assistant 片段和 process block。

process block 自身也必须显示工具执行耗时：`Ran 1 command · 4m 33s` 表示已完成工具调用的总耗时；运行中显示 `Running 1 command (1 active) · 1m 12s`，并每秒刷新一次。历史 timeline 从 `tool_call.timestamp` 到 `tool_result.timestamp` 计算；实时 SSE 从 `tool_started` 接收时间开始计时，`tool_finished.durationMs` 优先作为完成耗时来源。

边界规则：

- 如果没有 user 消息（例如独立 compaction 状态消息），保持原有单条消息渲染。
- 如果全局 `running=true`，最后一个 user turn 视为活跃 turn，不做默认折叠；在该 turn 的输出顶部展示 `已处理 <duration>` 并每秒刷新运行时长，更早的 turn 仍可折叠。
- 折叠只影响展示层，不改变 JSONL timeline、session detail 或续聊上下文。
- 颜色、边框和文字必须使用 Ant Design token，亮色/暗色主题都要可读。
- 窄屏时消息列必须随视口收缩并保留水平 padding；右侧 thread rail 在 `md` 以下隐藏，避免把消息内容挤到无 padding 状态；Markdown 长路径、代码块和表格不得撑出横向溢出。
- 消息区离开底部时，在 composer 正上方居中显示圆形滚动到底部按钮；按钮用 opacity + transform 淡入淡出，点击后复用现有直接滚到底部逻辑，不额外持久化任何 UI-only 滚动状态。
- `New Chat` 是创建新的独立 session，不属于当前 running turn 的输入或 stop 控制；即使当前选中的线程处于 `Running`，按钮也必须可用。后端 active worker 和 busy 保护按 `session_key` 隔离，只有同一个 session 的并发输入需要进入 guide/queue 或 busy 分支。

测试方案：

- 单元/组件：覆盖 completed turn 默认只显示最终输出和 `已处理` 摘要，展开后恢复 process block 与中间 delta。
- E2E：在 `web/tests/ui/agent-chat.spec.ts` 中覆盖 Web stream 完成态和 history timeline 完成态；运行中 history refresh 用例必须保持当前 turn 展开，并断言顶部 `已处理 <duration>` 会继续更新。
- E2E 额外覆盖 640px 视口，断言消息区没有横向溢出且 message track 与滚动区之间保留左右 padding。
- E2E 覆盖消息区离开底部时滚动按钮淡入、位置居中在 composer 上方、点击后滚到底部并淡出。
- E2E 覆盖选中 running 线程时 `New Chat` 仍可点击并创建新 session。
- human_tests：在 `human_tests/agent-session-persistence.md` 增加完成态折叠回归，按真实 WebUI 或 Playwright mock 逐条执行。
- Review/Fix/Test：两轮复核折叠默认态、展开态、运行态、暗色主题、历史深链与现有 process block 交互。

## 方案

### 0. Loop 下沉，Channel View 上提

核心模型调整：

```text
Conversation / AgentSession
  owns: loop state, run state, turn queue, context, canonical timeline

ChannelBinding
  owns: provider/channel identity, delivery cursor, card/message projection state

ChannelView
  owns: Web thread view, Feishu card view, Weixin message view, API snapshot
```

`AgentSession` 不再直接认为自己属于 Web 或 IM。它只接受 `UserInput`，运行 Agent Loop，维护唯一 `AgentRunState`，并产出 canonical timeline events。所有通道展示都从 run state + timeline/progress fan-out 得到自己的 projection。

建议定义 `ConversationChannelBinding`：

```json
{
  "conversation_id": "agent-session-key",
  "channel": "web|feishu|weixin|api",
  "provider_id": "feishu-workspace-a",
  "target_id": "chat_id/open_id/thread_id",
  "view_state": {
    "web_thread_id": "...",
    "im_card_message_id": "...",
    "last_delivered_event_id": 123
  },
  "enabled": true
}
```

几个关键规则：

- Web UI 创建或打开一个 conversation 时，注册/复用 `web` binding。
- IM 消息命中同一个 conversation 时，注册/复用对应 `im` binding。
- 任何 channel 发来的用户消息都先写入同一 canonical timeline，再进入同一 turn queue。
- Agent progress/final reply 由 timeline fan-out 分发到所有 enabled bindings。
- 每个 binding 自己维护 delivery cursor，避免重复更新飞书卡片或重复推送 Web SSE。

### 0.1 跨通道同对话 fan-out 场景

目标场景：

1. 用户在飞书里发送消息，命中 conversation A。
2. 系统为 conversation A 绑定 `feishu` channel，创建/更新飞书 progress card。
3. 用户在 Web UI 打开 conversation A，Web 注册 `web` binding，并加载同一 timeline。
4. 用户接着在 Web UI 发送新消息。
5. 该消息写入 conversation A 的 canonical timeline，并进入同一 AgentSession turn queue。
6. Agent 运行时产生的 `tool_call/plan/assistant_delta/assistant_final` 同时投影到：
   - Web UI thread/process panel
   - 飞书 progress card/final card
7. Web 刷新或飞书卡片重试更新时，都以 binding cursor + canonical timeline 恢复，不重新跑 Loop。

这个场景要求“触发通道”只作为 user input 的来源字段存在，例如：

```json
{
  "event_type": "user_message",
  "source_channel": "web",
  "conversation_id": "A",
  "content": "继续刚才的任务"
}
```

但后续 `assistant_final` 不只回到 `source_channel=web`，而是 fan-out 到 conversation A 的所有 active bindings。

### 0.2 ChannelBinding 与 turn 并发边界

同一个 conversation 的 Loop 仍应串行化或显式排队，避免 Web 和 IM 同时发消息时上下文顺序不确定：

- `AgentSession` 维护 turn queue。
- 每条 user message 有单调递增 `event_id` / `turn_id`。
- Web/IM 同时输入时，按进入 canonical timeline 的顺序排队。
- 被排队的输入也 fan-out：另一个通道应能看到“用户在 Web/IM 追加了消息，等待当前 turn 完成”。
- 如果当前 runner 支持 interrupt/queue/stop，这些控制命令也应作用于 conversation，而不是只作用于某个 channel 的局部状态。

### 0.3 统一 AgentRunState

运行状态需要和 timeline 一样下沉到底层：

```text
AgentRunState
  queued
  running
  waiting_for_tool
  waiting_for_user
  compacting
  completed
  failed
  cancelled
```

状态事件也写入 canonical timeline，例如：

```json
{
  "event_type": "run_state_changed",
  "conversation_id": "A",
  "turn_id": "turn-42",
  "agent_kind": "builtin|external_runner|codex|chatgpt_web",
  "state": "running",
  "source_channel": "web"
}
```

注意这里的 `source_channel` 只表达“这次输入来自哪里”，不决定状态展示范围。Web thread、IM card、API session list、Schedule run detail 都应该从同一 `AgentRunState` 投影：

- Web list 显示 running，IM card 也显示 running。
- External Runner 进入 waiting/failed，Web 和 IM 都能看到同一状态。
- 内置 Agent compaction/tool 状态，不因为入口是 IM 而只在 IM card 可见。
- stop/cancel/guide/queue 等控制命令作用于 `conversation_id + turn_id`，所有 channel view 同步更新。

### 0.4 IM Card 不再绑定“本次请求”

飞书 progress card 的生命周期应从“IM 请求路径创建的临时 card”调整为“conversation 的 IM binding projection”：

- IM 首次触发 conversation：创建 card，保存 `im_card_message_id` 到 binding state。
- Web 后续触发同一 conversation：如果存在 enabled IM binding，则复用或更新这张 card。
- 如果没有可更新 card，可以按通道策略创建一张新的 status card，并把新 card id 写回 binding。
- final reply 到达时，Web 和 IM 都从同一个 `assistant_final` event 更新最终展示。
- 如果 IM card 更新失败，只影响该 binding 的 delivery 状态，不影响 Loop 和其他通道展示。

### 1. 引入统一 AgentRunTimeline 语义

不一定立即新增大模块，但数据契约要先收敛：

- `ConversationRecorder` 写入的是 canonical timeline。
- `AgentTurnProgressEvent` 是运行时事件流。
- Web SSE、IM progress card、JSONL persistence 都应该由 canonical timeline 或 runtime event 的同一映射层产生。

建议定义一个小型 adapter 层：

```text
AgentTurnProgressEvent
  -> ConversationEvent
  -> ChannelFanout
  -> Web timeline view
  -> IM progress card view
  -> persisted JSONL
```

短期可以先复用现有 JSONL event type，不急着迁移文件格式。关键是 `AgentTurnProgressEvent` 不再只发给单个 `progress_sender`，而是进入一个按 conversation fan-out 的 dispatcher；Web SSE 和 IM card 都是 dispatcher 的 subscribers。

### 2. SessionInfo / SessionDetail 带上 history_path

当 `AgentSession` 持有 recorder 时，应把 recorder file path 暴露到：

- `SessionInfo`
- `SessionDetail`
- active/running `/sessions/all` item

这样 Web Chat 即使打开 active/idle session，也能知道它有对应 JSONL event timeline。

建议字段：

```json
{
  "session_key": "im:provider:user",
  "status": "active",
  "history_path": ".../session-im-provider-user-xxxx.jsonl",
  "has_timeline": true
}
```

### 3. `/sessions/all` 合并 active 与 history，而不是互斥

当前 active key 存在时跳过 history row。应改为：

- 先扫描 history files，按 session_key 找最新 history_path。
- active/running/idle session item 与 history summary 合并。
- 最终只保留一条 thread，但包含 `history_path`。

合并优先级：

- runtime state：`running/state/work_dir/token/context/runner`
- persisted timeline：`history_path/title/start_time/last_active_time/message count`
- title：显式 title > active title > history title > first user message

这样不会出现两个重复 thread，也不会丢 timeline。

### 4. Web Chat 优先消费 timeline events

前端打开 thread 时：

- 如果 thread 有 `history_path`，先请求 `/sessions/history/:path`。
- 用 `historyEventsToMessages()` 恢复聊天气泡。
- 用 `historyEventsToTelemetry()` 恢复 tool/plan/compaction/process。
- 再用 `/sessions/:key` 或 thread summary 补充 active runtime 状态。

运行中 Web 自己发起的 turn 仍继续使用 SSE 增量更新；IM 发起或刷新后恢复的 running turn 必须继续消费同一条 `history_path` 的 append-only timeline。只要 `run_state` 或 thread summary 的 `running=true` 表示 queued/running/waiting，Web view 就要周期性重新读取 history events，直到看到 completed/failed/cancelled。

刷新恢复有一个硬约束：如果底层 Loop 仍在 running，消息区不能只剩用户消息。即使 JSONL 暂时只写入了 `user_message`、没有新的 `run_state_changed`、或者还没有新的 `tool_call`，只要 thread summary 仍标记 running，视图层也必须在最后一个 user message 后补一个 assistant running placeholder，用来承载后续 process steps。后续 `tool_call/tool_result/plan/compaction` append 到 timeline 后，placeholder 原位更新为过程卡片；最终 `assistant_message` 写入后替换为正式回答。

关键点：Web Chat 不应因为 `view=active` 就放弃 history events。active/history 应该只是状态，不是能否看到过程的开关。

### 5. IM Progress Card 与 Web Timeline 共用映射

`run_progress_event_coalescer()` 当前直接把 `AgentTurnProgressEvent` 应用到 `ImAgentProgressRegistry`。建议提取共享 mapper：

```text
AgentTurnProgressEvent -> TimelineEventPatch
TimelineEventPatch -> ImProgressSnapshot
TimelineEventPatch -> WebRunTelemetry
TimelineEventPatch -> ConversationEvent
```

短期不需要一次性重写 IM card，只要保证新的 canonical event type 不让 IM/Web 各自理解一套私有格式。

### 5.1 ChannelFanoutDispatcher

建议新增一个轻量 dispatcher，负责把 canonical timeline patch 投递到所有绑定通道：

```text
ConversationEvent / TimelineEventPatch
  -> load enabled ChannelBinding by conversation_id
  -> deliver to Web subscribers
  -> deliver to IM card updater
  -> update binding delivery cursor
```

Web 通道：

- running thread 有 SSE/websocket subscriber 时，实时推送。
- 没有 subscriber 时，只推进 persisted timeline；下次打开 Web 时回放。

IM 通道：

- 有 `im_card_message_id` 时更新同一张 progress card。
- 没有 card 但策略允许时创建 status card。
- 更新失败记录到 binding，不回滚 canonical timeline。

这样 Web 发消息时也能更新飞书卡片，IM 发消息时也能更新 Web thread，核心行为不再依赖请求入口。

### 6. External Runner 事件归一化

External Runner 当前把 events 放进 tool_result blob。后续要补：

- 解析 `ExternalCliRunResult.events`
- 解析 `artifacts.normalized_events`
- 生成标准 `ConversationEvent`

建议映射：

| Runner Event | ConversationEvent |
| --- | --- |
| run started | `tool_call` 或 `runner_started` |
| tool started | `tool_call` |
| tool finished | `tool_result` |
| plan update | `plan_updated` |
| assistant delta/final | `assistant_message` 或 streaming delta event |
| error | `turn_failed` / failed `tool_result` |

如果 runner events 的 schema 尚不稳定，可以先写 `runner_event`，但 Web helper 必须能解析并展示。

External Runner、内置 Agent、Codex、ChatGPT Web 不应该各自暴露互不相通的状态枚举。不同 Agent 的私有事件可以先 adapter 成 canonical events：

```text
Runner private state -> AgentRunState
Runner private event -> ConversationEvent
Runner artifact      -> Timeline attachment/artifact reference
```

视图层不直接理解 runner 私有状态，只理解 `AgentRunState` 和 `ConversationEvent`。

### 7. 数据模型收敛策略

不为旧数据和旧 active/history 分裂路径额外设计兼容层。需要新增或调整字段时，直接以 canonical 模型为准：

- `history_path: string | null`
- `has_timeline: boolean`
- `timeline_event_count: number`
- `run_state: AgentRunState`
- `channel_bindings: ChannelBindingSummary[]`

现有 `messages` 可以作为 timeline projection 的派生结果保留，但不再作为完整 session detail 的权威来源。

删除或弱化的语义：

- 不再把 `/sessions/:key` 视为完整详情唯一来源。
- 不再用 active/history 两个 list item 表达同一会话。
- 不再为“只有 messages、没有 timeline”的历史路径增加复杂 fallback；缺失 timeline 时明确显示数据不可回放即可。

## 推荐实施步骤

### Phase 1：让 Web 能看到 IM 内置 Agent 的完整过程

1. `AgentSession` / `SessionInfo` / `SessionDetail` 暴露 recorder `history_path`。
2. `/sessions/all` 合并 active session 与最新 history file。
3. `AgentChatSection` 打开带 `history_path` 的 thread 时优先加载 history events。
4. 保留 active detail 作为运行状态补充。
5. 引入 conversation-level channel binding 元数据，至少能记录 Web thread 与 IM card 的绑定关系。
6. 引入统一 `AgentRunState` projection，让 Web session list、Web thread、IM card 都从同一运行状态更新。
7. 补单元测试和 Web helper 测试。

验收标准：

- IM 触发内置 Agent + tool call。
- Web Chat 打开同一 thread。
- 页面展示 user、assistant final、tool call/process、plan/compaction telemetry。
- 刷新页面后仍能看到完整过程。
- Web 在同一 thread 发送后续消息时，飞书绑定 card 也更新状态和最终回复。
- Web、IM 对同一 conversation 的 running/completed/failed 状态保持一致。

### Phase 2：External Runner timeline 归一化

1. 给 `ExternalCliRunEvent` 到 `ConversationEvent` 增加 mapper。
2. 给 External Runner 私有状态到 `AgentRunState` 增加 mapper。
3. `record_external_cli_result()` 不只保存 `event_types`，还写入可回放 timeline events 和 run state changes。
4. `session_state.json` 继续保存轻量 thread metadata，不作为过程或运行状态数据源。
5. Web Chat 对 external runner history 使用同一 `historyEventsToTelemetry()` 和 `AgentRunState` projection。

验收标准：

- Codex/ChatGPT Web Runner 从 IM 或 Web 发起后，Web Chat 能看到 normalized tool/plan/progress。
- 刷新页面后不退化成“用户消息 + 最终输出”。
- Codex/ChatGPT Web Runner 的 running/failed/completed 状态在 Web 与 IM 绑定视图中一致。

### Phase 3：统一实时订阅

1. 增加按 `session_key` 订阅 timeline/progress 的 Web endpoint。
2. IM 发起的 running turn 在 Web 打开时，也能看到实时过程，而不是等待结束后回放。
3. IM progress card 与 Web process panel 使用同一状态快照。
4. 引入 `ChannelFanoutDispatcher`，让 Web/IM/API projection 都订阅 conversation timeline，而不是每条入口链路单独挂一个 sender。

验收标准：

- IM 发起长任务，Web 中途打开同 thread 能看到 running process。
- Web 发 guide/queue/stop 与 IM 发 guide/queue/stop 使用同一底层 session。
- Web 发普通消息触发同一 conversation 的新 turn，IM progress card 同步展示 running/final。

## 测试计划

### 单元测试

- `sessions_all_merges_active_session_with_history_path`
- `session_detail_exposes_history_path_when_recorder_exists`
- `history_events_to_telemetry_parses_tool_plan_compaction`
- `external_runner_events_are_normalized_to_conversation_events`
- `channel_bindings_fan_out_web_turn_to_im_card`
- `channel_bindings_fan_out_im_turn_to_web_thread`
- `conversation_turn_queue_orders_web_and_im_inputs`
- `agent_run_state_projects_to_all_bound_channels`
- `external_runner_state_maps_to_canonical_agent_run_state`

### E2E 测试

- `test_im_agent_web_timeline_visibility.sh`
  - 启动 mock model。
  - 通过 IM event loop 注入一条会触发 tool call 的消息。
  - 等 turn 完成。
  - 调 `/api/im-gateway/agent/sessions/all`，断言同一 session item 有 `history_path`。
  - 调 `/sessions/history/:path`，断言存在 `tool_call/tool_result/assistant_message`。
  - 用浏览器打开 Agent Chat，断言 process panel 有 tool step。

- `test_external_runner_web_timeline_visibility.sh`
  - 使用 mock external runner 输出 normalized events。
  - 通过 IM 或 Web Chat Gateway 触发。
  - 断言 Web Chat 刷新后仍显示 runner progress/tool events。
  - 断言刷新时如果最新 turn 仍 running，最后一个 user message 下方一定存在 assistant running/process 卡片。

- `test_cross_channel_conversation_fanout.sh`
  - 通过 IM 注入一条消息，绑定 conversation A 与飞书 card。
  - 通过 Web UI/API 对同一个 conversation A 发送后续消息。
  - 断言 canonical timeline 中按顺序存在 IM user message、Web user message、assistant/tool events。
  - 断言 Web thread 和 IM card 都收到 running 状态与 final reply。

- `test_cross_agent_run_state_unification.sh`
  - 分别用内置 Agent 与 mock External Runner 触发同一类 conversation run。
  - 断言两者都写入 canonical `run_state_changed` events。
  - 断言 Web list、Web thread、IM card 从同一 state projection 展示 queued/running/completed/failed。

### human_tests

在 `human_tests/im-gateway-agent.md` 增加用例：

- IM 通道触发内置 Agent 后，Web Chat 打开同一 thread 能看到工具调用过程。
- Web 刷新后仍能看到同一过程。
- External Runner 从 IM 通道触发后，Web Chat 能看到 normalized progress。
- IM 触发同一 conversation 后，Web UI 继续发送消息，飞书 card 同步更新运行状态与最终回复。
- Web UI 触发同一 conversation 后，已绑定的 IM 通道能看到状态卡片或最终消息更新。
- 内置 Agent 与 External Runner 的 running/failed/completed 状态在 Web 和 IM 两个视图中一致。

## 风险与边界

- 历史文件可能被 retention 清理：active session 应允许 `history_path` 为空，此时 Web 退回 messages-only，但状态要明确。
- running turn 的 JSONL 可能正在写：读取 events 要支持 append-only 部分可读，必要时做 lossy loading。
- External Runner 事件 schema 不统一：先定义 canonical mapper，再接各 adapter。
- 同一 session_key 多 runner 并存：需要保留 `adapter/runner_id` 维度，避免内置 Agent 与外部 Runner 的 timeline 串线。
- Runner-call 子线程仍需保持隔离：`runner-call:*` 可以继续从顶层列表隐藏，但父消息应能链接到子 timeline。
- 多通道 fan-out 会带来重复投递风险：必须用 binding-level `last_delivered_event_id` 或幂等 key 控制 Web SSE/IM card 更新。
- IM card 可能被用户删除、过期或无权限更新：该 binding 应进入 degraded 状态，Web 与 canonical timeline 不受影响。
- Web 与 IM 同时发消息时必须有明确排队策略；不能让两个 turn 并行改同一个 context。
- 不做旧数据兼容会让部分历史 session 只显示不可回放或需要重新索引，这是可接受的简化边界。

## Open Questions

1. Web Chat 默认打开 thread 时，是不是永远优先 timeline events，再补 active status？建议是。
2. External Runner 的 delta 是否需要逐条展示，还是只展示 tool/plan/final？建议先展示结构化过程，delta 可折叠或聚合。
3. IM progress card 是否也要支持从 JSONL 回放？建议不需要，IM card 只展示实时与最终态，Web 负责完整回放。
4. `session_state.json` 是否长期保留 messages？建议只保留轻量 metadata，过程数据和运行状态以 canonical timeline 为准。
5. 飞书 card 被删除或更新失败时，是否自动重建新 card？建议按 provider 配置决定，但 binding 必须记录失败原因。
6. 同一个 conversation 绑定多个 IM chat/open_id 时，是否全部 fan-out？建议默认只 fan-out enabled binding，并提供用户可见的绑定管理。
