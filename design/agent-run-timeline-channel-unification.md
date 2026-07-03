# Agent Run Timeline 与多通道展示统一方案

## 背景

Bifrost Agent 有多个触发入口：Web Agent Chat（`/api/agent/chat/stream`、`/api/im-gateway/agent/chat`）、IM Gateway（飞书、微信）、External Runner（Codex、ChatGPT Web、自定义 CLI Runner）、以及定时任务与手动触发。产品预期是：这些入口只是消息投递与结果展示通道不同，底层 Agent Loop、工具调用、plan、compaction、token/context 状态应该落在同一条可回放的 timeline 上；同一 conversation 上任何通道发起的输入和 Agent 产出的过程/最终结果都应该 fan-out 到该 conversation 已绑定的全部通道。

现状偏差：Web Chat 打开 IM 触发的同一 thread 时，常常只能看到 user message 与 assistant final response，工具/plan/compaction/context 事件缺失；`session_state.json` 与 canonical JSONL timeline 的读写路径分裂，`/sessions/all` 曾出现 active/history 互斥展示；External Runner 的私有 event 塞在 `tool_result` blob 里未归一化；不同 Agent/Runner 的 running 状态各自维护，导致 Web 认为 idle 但 IM card 仍显示 running 之类的漂移。

本方案把 Agent Loop 下沉到 conversation/session 层，视图上提到 channel projection 层：Web、飞书、微信、API 都只是同一个 conversation 的输入端与展示端。历史数据兼容不作目标，允许直接收敛到新的 canonical timeline + channel binding 数据模型。

（现状更新 2026-06-16：`/sessions/all` 已为 active/running/idle item 合并 `history_path` 与 `has_timeline`，前端 `AgentChatSection` 已通过 `historyEventsToMessages` / `historyEventsToTelemetry` 消费历史 timeline，Phase 1 的步骤 1-4 已 shipped。剩余差距集中在跨通道 binding/fan-out、统一 `AgentRunState`、External Runner event 归一化。）

## 用户目标验证清单

### 必须实现

- 同一 `session_key` 的同一 turn 只写一条 canonical timeline；Web/IM/API/Schedule 都从该 timeline 投影自己的展示。
- `SessionInfo` / `SessionDetail` / `/sessions/all` item 暴露 `history_path`、`has_timeline`；打开带 `history_path` 的 thread 时前端优先加载 `historyEventsToMessages` + `historyEventsToTelemetry`，再补 active runtime 状态。
- `/sessions/all` 合并 active/running/idle session 与 history summary，不再互斥；title 优先级 `显式 title > active title > history title > first user message`。
- Web Chat 刷新后仍能看到完整历史工具调用、plan、compaction 与最终输出。
- running turn 刷新恢复时，若 thread summary 仍 `running=true`，最后一个 user message 后必须补 assistant running placeholder 承载后续 process。
- IM Gateway 内置 Agent worker 与 Web SSE 复用同一 `AgentTurnProgressEvent → ConversationEvent → ChannelFanout` 映射；不再各自维护私有格式。
- External Runner 私有事件（`ExternalCliRunEvent`、`artifacts.normalized_events`）经 mapper 变成标准 `ConversationEvent`（`tool_call/tool_result/plan_updated/assistant_message/turn_failed/run_state_changed`）后写入 canonical timeline。
- 统一 `AgentRunState { queued|running|waiting_for_tool|waiting_for_user|compacting|completed|failed|cancelled }`；所有视图从同一状态机感知，`stop/cancel/guide/queue` 作用于 `conversation_id + turn_id`。
- `ChannelBinding { conversation_id, channel, provider_id, target_id, view_state, enabled }` 记录 Web thread / 飞书 card / 微信 message 等 binding；`ChannelFanoutDispatcher` 把 timeline patch 投递到 conversation 的全部 enabled bindings，各自维护 `last_delivered_event_id` 避免重复投递。
- Web 发消息也能更新飞书卡片，IM 发消息也能更新 Web thread。
- 运行态真源按优先级收敛：live runtime registry > canonical JSONL 终态 > persisted `session_state`；内置 `bifrost_agent` 陈旧 running 无 live 证明时投影为 ended/completed 并落盘修复；非内置 runner 无 JSONL 终态且无 live registry 证明时保留 persisted `running` 表达跨装置异步语义。
- `/stop` 先请求 live runtime 停止；未命中但 persisted 为陈旧 running 时修复为 terminal 并在响应暴露修复计数。
- Web AgentChat 完成态 loop 默认折叠（`已处理 <duration>` 摘要，可展开），运行态最后 turn 不折叠且顶部 `已处理 <duration>` 每秒刷新；`Ran 1 command · 4m 33s` / `Running 1 command (1 active) · 1m 12s`。
- 消息区图片（Markdown 与 `content_parts`）注册为当前会话图片序列，点击打开全屏浮窗，支持左右/键盘切换、遮罩关闭、关闭后 `scrollIntoView`。
- 窄屏（<`md`）隐藏右侧 thread rail、消息区保留水平 padding、长路径/代码/表格不横向溢出。
- 消息区离开底部时 composer 上方居中显示圆形滚动按钮（opacity+transform 淡入淡出），点击滚到底部。
- 选中 running 线程时 `New Chat` 仍可点击创建新 session；busy 保护按 `session_key` 隔离。
- WebUI slash runner 触发的 runner-call 收到 `runner_call_finished status != completed/success/succeeded` 时抛错，把 user/assistant chip 标 `failed`，正文替换错误文本；成功路径不变。
- External CLI `run_command` 等待子进程时同时监听 `stop_requested` marker；命中后终止进程组并写 `ExternalCliRunStatus::Stopped` + stopped stderr + stopped run event + result。
- `chatgpt_web::run_adapter` 认证等待阶段用 `tokio::select!` 同时监听 stop marker；被打断时按 profile 清理 managed browser。
- `read_run_detail` 优先返回 `result.json.response`，仅无 result response 时回退 stdout/events；terminal 状态非 succeeded 且 final response 为空时按 `run_failed event -> stderr -> stdout -> 默认错误文案` 提升为可见 response。
- WebUI 主 Agent Chat stream `run_finished`/`turn_finished` 非成功值按失败处理，状态进 Error、线程列表停止 running。
- Codex adapter 默认追加 `--config service_tier="fast"`，若 runner 显式配置 `service_tier=...` 则保留 override。

### 必须不破坏

- 已 shipped 的 `history_path` / `has_timeline` 字段、`historyEventsToMessages/Telemetry` 协议与前端 helper。
- 现有 IM progress card、Web SSE、JSONL persistence 的 event type 命名（短期不改文件格式）。
- runner-call 子线程隔离（`runner-call:*` 从顶层列表隐藏），仅新增父子 timeline 链接。
- 已 shipped 的 SSE 帧频、IM card 更新 debounce、chat gateway 现有语义。
- `session_state.json` 作为线程索引与展示摘要缓存的作用；不再作为消息/运行状态事实源，但不删除文件。
- `runner_call_finished` 成功路径继续 `response` 替换 assistant 正文与 `success` 标记。
- Codex 用户显式 `service_tier` override 保留。

### 必须真实验证

- IM 触发内置 Agent + tool call 后 Web Chat 打开同一 thread 能看到 user、assistant final、tool call/process、plan/compaction telemetry；刷新页面后仍完整。
- Web 在同一 thread 发送后续消息时，飞书绑定 card 同步更新状态与最终回复；反向亦然。
- Codex/ChatGPT Web Runner 从 IM 或 Web 触发后 Web Chat 能看到 normalized tool/plan/progress；刷新不退化。
- Web、IM 对同一 conversation 的 running/completed/failed 状态一致。
- IM 发起长任务，Web 中途打开同 thread 能看到实时 running process。
- `/stop` 触发 live runtime 停止；对陈旧 running persisted state 修复响应包含修复计数。
- Codex 真实链路 `service_tier` 默认注入；traex/chatgpt_web `/stop` 后 stream 归入 `run_finished(status=stopped)` 且无进程残留。
- WebUI E2E：`AI Agent Chat marks runner-call finished failures as failed`；外部 runner terminal failed 状态 UI 展示 Error。
- 折叠、图片全屏、窄屏 padding、滚动按钮、New Chat 可用性由 Playwright 覆盖。

## 产品语义

### 通道与会话解耦

- `Conversation / AgentSession` owns：loop state、run state、turn queue、context、canonical timeline。
- `ChannelBinding` owns：provider/channel identity、delivery cursor、card/message projection state。
- `ChannelView` owns：Web thread view、飞书 card view、微信 message view、API snapshot。

`AgentSession` 不再直接认为自己属于 Web 或 IM，只接受 `UserInput`，运行 Loop，维护唯一 `AgentRunState`，产出 canonical timeline events。所有通道展示都从 run state + timeline/progress fan-out 得到自己的 projection。

### 运行态一致性真源

按优先级：

1. 当前进程 live runtime registry（内置 Agent active turn、worker stop registry、external CLI worker registry）是“仍在运行”的唯一实时证明。
2. canonical JSONL timeline 的最新终态 `run_state_changed`（`completed/failed/stopped/timed_out`）是“已经结束”的持久化事实源。
3. `/sessions/all` 合并 persisted `session_state` 时，如果显式 `history_path` 指向的 JSONL 已有终态，按 JSONL 终态投影并使用 JSONL 终态时间排序。
4. 内置 `bifrost_agent` persisted `status:"running"` 若无 live 证明视为陈旧，投影为 ended/completed。
5. 非内置 runner adapter（`codex`/`chatgpt_web`/自定义）无 JSONL 终态且无 live registry 证明时保留 persisted `running`。
6. `/stop` 未命中 live 但 persisted 为陈旧 running 时同步修复 `session_state.json` 并在响应暴露修复计数。
7. 不做旧版 `session_state` 历史文件扫描或 alias 兼容。

### 完成态 Loop 折叠展示

Web Agent Chat 按 `user_message` 切 turn；已结束 turn 默认收起中间过程，只显示 `已处理 <duration> >` 摘要行；运行态最后 turn 保持展开并 `已处理 <duration>` 每秒刷新；process block 显示 `Ran 1 command · 4m 33s` / `Running 1 command (1 active) · 1m 12s`。折叠只影响展示，不改变 JSONL、session detail、续聊上下文；颜色使用 Ant Design token 亮暗主题皆可读。

### 对话图片全屏预览

Markdown 图片与多模态 `content_parts` 图片按渲染顺序收集为会话图片序列；点击打开全屏浮窗（opacity+scale 过渡），支持左右按钮 + 键盘 `ArrowLeft/ArrowRight` 切换、`Escape` 关闭、遮罩关闭；关闭时按稳定 image id `scrollIntoView({ block: "center" })` 回到原位置。

## 技术细节

### 1. Loop 下沉，Channel View 上提

```text
Conversation / AgentSession
  owns: loop state, run state, turn queue, context, canonical timeline

ChannelBinding
  owns: provider/channel identity, delivery cursor, card/message projection state

ChannelView
  owns: Web thread view, Feishu card view, Weixin message view, API snapshot
```

`ConversationChannelBinding` 结构：

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

Web 打开 conversation 复用/注册 `web` binding；IM 消息命中 conversation 复用/注册 `im` binding；任何 channel 发来的 user message 先写 canonical timeline 再进 turn queue；Agent progress/final reply 由 timeline fan-out 分发；binding 各自维护 delivery cursor 避免重复更新。

### 2. 跨通道同对话 fan-out 场景

1. 用户在飞书发消息命中 conversation A。
2. 系统为 A 绑定 `feishu` channel，创建/更新 progress card。
3. 用户在 Web UI 打开 A，Web 注册 `web` binding，加载同一 timeline。
4. 用户在 Web 发新消息，写入 A 的 canonical timeline并进入同一 turn queue。
5. Agent 运行时产生的 `tool_call/plan/assistant_delta/assistant_final` 同时投影到 Web thread/process panel 与飞书 progress card/final card。
6. Web 刷新或飞书卡片重试更新时都以 binding cursor + canonical timeline 恢复，不重新跑 Loop。

`user_message` 事件带 `source_channel` 表达来源；`assistant_final` fan-out 到 conversation 所有 active bindings。

### 3. ChannelBinding 与 turn 并发边界

同一 conversation 的 Loop 串行/显式排队：

- `AgentSession` 维护 turn queue。
- 每条 user message 带单调递增 `event_id`/`turn_id`。
- Web/IM 同时输入按进入 canonical timeline 顺序排队。
- 被排队的输入 fan-out：另一通道能看到“用户在 Web/IM 追加了消息，等待当前 turn 完成”。
- interrupt/queue/stop 作用于 conversation，不作用于某个 channel 局部状态。

### 4. 统一 AgentRunState

```text
queued
running
waiting_for_tool
waiting_for_user
compacting
completed
failed
cancelled
```

canonical timeline `run_state_changed` 事件：

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

`source_channel` 只表达来源，不决定状态展示范围；Web list、IM card、API session list、Schedule run detail 都从同一状态投影；stop/cancel/guide/queue 作用于 `conversation_id + turn_id`。

### 5. IM Card 不再绑定“本次请求”

飞书 progress card 生命周期从“IM 请求路径创建的临时 card”调整为“conversation 的 IM binding projection”：首次触发创建 card 保存 `im_card_message_id` 到 binding state；后续任何通道触发同一 conversation 复用/更新该 card；无可更新 card 时按通道策略新建 status card 并写回 binding；final reply 到达时所有 binding 更新；更新失败只影响该 binding delivery state，不影响 Loop 与其他通道。

### 6. AgentRunTimeline 语义与映射层

```text
AgentTurnProgressEvent
  -> ConversationEvent
  -> ChannelFanout
  -> Web timeline view
  -> IM progress card view
  -> persisted JSONL
```

- `ConversationRecorder` 写 canonical timeline。
- `AgentTurnProgressEvent` 是运行时事件流。
- `run_progress_event_coalescer()` 短期继续，但抽出共享 mapper：`AgentTurnProgressEvent -> TimelineEventPatch -> {ImProgressSnapshot, WebRunTelemetry, ConversationEvent}`。
- 短期复用现有 JSONL event type，不急着迁移文件格式。

### 7. SessionInfo / SessionDetail 与 /sessions/all 合并

`SessionInfo`、`SessionDetail`、active/running/idle `/sessions/all` item 携带：

```json
{
  "session_key": "im:provider:user",
  "status": "active",
  "history_path": ".../session-im-provider-user-xxxx.jsonl",
  "has_timeline": true
}
```

`/sessions/all` 合并优先级：

- runtime：`running/state/work_dir/token/context/runner`
- persisted timeline：`history_path/title/start_time/last_active_time/message count`
- title：显式 title > active title > history title > first user message

不出现重复 thread，也不丢 timeline。

### 8. Web Chat 优先消费 timeline events

前端打开 thread 时：

- 若有 `history_path`，先 `/sessions/history/:path` + `historyEventsToMessages()` + `historyEventsToTelemetry()`。
- 再 `/sessions/:key` 或 thread summary 补 active runtime 状态。
- Web 自己发起的 running turn 继续 SSE 增量；IM 发起或刷新恢复的 running turn 周期性重读 history events，直到看到 completed/failed/cancelled。
- 刷新恢复硬约束：若底层 Loop 仍 running 且 JSONL 只有 `user_message`，视图层必须在最后 user 后补 assistant running placeholder 承载后续 process；后续 `tool_call/tool_result/plan/compaction` append 后就地更新，`assistant_message` 写入后替换为正式回答。

### 9. External Runner 事件归一化

映射：

| Runner Event | ConversationEvent |
| --- | --- |
| run started | `tool_call` 或 `runner_started` |
| tool started | `tool_call` |
| tool finished | `tool_result` |
| plan update | `plan_updated` |
| assistant delta/final | `assistant_message` 或 streaming delta event |
| error | `turn_failed` / failed `tool_result` |

`record_external_cli_result()` 不只保存 `event_types`，还写入可回放 timeline events 与 `run_state_changed`。`session_state.json` 只保轻量 thread metadata。Web Chat 对 external runner history 使用同一 `historyEventsToTelemetry()` 与 `AgentRunState` projection。不同 runner 私有状态经 adapter 映射为统一 `AgentRunState`。

### 10. ChannelFanoutDispatcher

```text
ConversationEvent / TimelineEventPatch
  -> load enabled ChannelBinding by conversation_id
  -> deliver to Web subscribers
  -> deliver to IM card updater
  -> update binding delivery cursor
```

Web：running thread 有 SSE/websocket subscriber 时实时推；无 subscriber 时推进 persisted timeline，下次打开回放。
IM：有 `im_card_message_id` 时更新同一 card；无 card 但策略允许时新建 status card；更新失败记录 binding，不回滚 canonical timeline。

### 11. Web Runner Call 失败状态投影

`runRunnerCallStream` 消费 `/api/im-gateway/chat/runner-calls/stream` NDJSON；收到 `runner_call_finished` 且 `status ∉ {completed,success,succeeded}` 时抛错。`useRunnerCallHandler` catch 分支把 user/assistant runner-call chip 标 `failed`，正文替换错误文本；成功路径不变。

### 12. External Runner SSE 停止与结果一致性

- external CLI `run_command` 等待子进程时同时监听 `stop_requested` marker；命中终止进程组并写 `ExternalCliRunStatus::Stopped` + stopped stderr + stopped run event + result。
- `chatgpt_web::run_adapter` 认证等待阶段 `tokio::select!` 同时监听 stop marker；被打断按 profile 清理 managed browser 避免 Edge 残留。
- `read_run_detail` 优先 `result.json.response`；无 result response 才回退 stdout/events。
- terminal 非 succeeded 且 final response 为空按 `run_failed event -> stderr -> stdout -> 默认错误文案` 提升为可见 response，覆盖 API / session detail / WebUI / IM card。
- WebUI 主 Agent Chat stream `run_finished`/`turn_finished` 非成功值按失败处理：正文展示 response/error，状态进 Error，线程列表停止 running；与 runner-call failure path 一致。
- Codex adapter 默认追加 `--config service_tier="fast"`；用户 override 保留。

## CLI / Admin API / Web

### CLI

- `bifrost agent chat/status/stop`：`/stop` 走统一 `request_agent_stop()` helper（见 `agent-session-context-restore.md`），同时通知 live runtime 与 external-cli stop marker。
- `bifrost agent runs list`：展示 canonical run + `AgentRunState`。

### Admin API

- `GET /_bifrost/api/im-gateway/agent/sessions/all`：合并 active/running/idle 与 history summary，item 含 `history_path`/`has_timeline`/`run_state`/`running`。
- `GET /_bifrost/api/im-gateway/agent/sessions/history/:path`：返回 JSONL events。
- `POST /_bifrost/api/im-gateway/agent/chat`：`/stop` 归入统一 helper；stream 支持 `run_state_changed`。
- `POST /_bifrost/api/im-gateway/chat/stream`：SSE 支持 `run_started` / `run_finished(status=...)`；`/chat/runs/{runId}/stop` 修复 stopped 状态语义。
- `GET /_bifrost/api/im-gateway/chat/runs/{runId}`：优先返回 `result.json.response`。
- ChannelBinding 相关未来 endpoint（planned）：`POST /agent/channel-bindings`、`GET /agent/conversations/{id}/bindings`。

### Web

- `AgentChatSection`：优先加载 history events + telemetry；running turn 补 placeholder；完成态 loop 默认折叠；`Ran/Running N command · <duration>`；图片全屏浮窗；窄屏 padding；滚动按钮；`New Chat` running 时可用。
- `useRunnerCallHandler`：runner-call finished 非成功状态标 failed。
- Playwright 覆盖：`web/tests/ui/agent-chat.spec.ts`。

## Sync 边界

- Canonical timeline JSONL 与 session_state 属于本机数据；不跨设备 sync。
- ChannelBinding 是本机 conversation 与本机 IM/Web 通道的映射，也不跨设备 sync（跨装置 conversation 恢复走独立设计）。
- 非内置 runner 的“跨装置 running”语义仅体现在 persisted state；本方案保留该 running 表达，不做跨装置聚合。

## 实现切分

### Phase 1：Web 能看到 IM 内置 Agent 完整过程

1. `AgentSession` / `SessionInfo` / `SessionDetail` 暴露 recorder `history_path`。（shipped）
2. `/sessions/all` 合并 active session 与最新 history file（active/idle item 已 fallback 到 `history_path`/`has_timeline`）。（shipped）
3. `AgentChatSection` 打开带 `history_path` 的 thread 时优先加载 history events。（shipped）
4. 保留 active detail 作为运行状态补充。（shipped）
5. 引入 conversation-level ChannelBinding metadata。（planned as of 2026-06-16）
6. 引入统一 `AgentRunState` projection。（planned）
7. 补单元测试与 Web helper 测试（部分 shipped：`historyEventsToTelemetry`/`historyEventsToMessages` 与 `/sessions/all` 已有单测；binding 测试待补）。

验收：IM 触发内置 Agent + tool call → Web Chat 打开同一 thread 能看到 user/assistant final/tool call+process/plan+compaction；刷新页面仍完整；Web 发消息时飞书 card 同步；running/completed/failed 一致。

### Phase 2：External Runner timeline 归一化

1. `ExternalCliRunEvent → ConversationEvent` mapper。
2. External Runner 私有状态 → `AgentRunState` mapper。
3. `record_external_cli_result()` 写可回放 timeline events + run state changes。
4. `session_state.json` 只保轻量 metadata。
5. Web Chat 对 external runner history 用同一 `historyEventsToTelemetry()` + `AgentRunState` projection。

验收：Codex/ChatGPT Web Runner 从 IM 或 Web 发起后 Web Chat 能看到 normalized tool/plan/progress；刷新不退化。

### Phase 3：统一实时订阅

1. 按 `session_key` 订阅 timeline/progress 的 Web endpoint。
2. IM 发起的 running turn Web 打开能看到实时过程。
3. IM progress card 与 Web process panel 使用同一状态快照。
4. `ChannelFanoutDispatcher`：Web/IM/API projection 都订阅 conversation timeline。

验收：IM 发长任务 Web 中途打开能看 running process；Web/IM 的 guide/queue/stop 使用同一底层 session；Web 普通消息触发同一 conversation 新 turn，IM progress card 同步。

### Phase 4：状态修复、折叠与图片浮窗

1. `/stop` 修复陈旧 running persisted 并暴露修复计数。
2. Web AgentChat 完成态折叠 + `已处理 <duration>` 摘要 + process block 耗时格式。
3. 图片全屏浮窗（Markdown + `content_parts`）+ 键盘 + 遮罩 + `scrollIntoView`。
4. 窄屏 padding、滚动按钮、`New Chat` running 可用。
5. runner-call failure 投影、External Runner SSE stop、Codex `service_tier="fast"` 默认注入。

## 测试方案

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
- `run_state_projection_prefers_live_registry_then_jsonl_terminal_then_persisted`
- `stop_repairs_stale_running_persisted_and_returns_fix_count`
- `read_run_detail_prefers_result_response_over_stdout`
- `codex_adapter_defaults_service_tier_fast_but_preserves_override`

### E2E 测试

- `test_im_agent_web_timeline_visibility.sh`：mock model + IM event loop 注入 tool call 消息；`/sessions/all` 断言 `history_path`；`/sessions/history/:path` 断言含 `tool_call/tool_result/assistant_message`；Playwright 断言 process panel 有 tool step。
- `test_external_runner_web_timeline_visibility.sh`：mock external runner 输出 normalized events；IM 或 Web Chat Gateway 触发；Web Chat 刷新后仍显示 runner progress/tool events；running turn 刷新时最后 user 下方必定存在 assistant running/process 卡片。
- `test_cross_channel_conversation_fanout.sh`：IM 注入命中 conversation A；Web 对同一 A 发送后续；canonical timeline 顺序含 IM user、Web user、assistant/tool；Web thread 与 IM card 都收到 running + final reply。
- `test_cross_agent_run_state_unification.sh`：内置 Agent 与 mock External Runner 分别触发同一类 run；断言两者都写 `run_state_changed`；Web list/Web thread/IM card 从同一 state projection 展示 queued/running/completed/failed。
- Playwright（`web/tests/ui/agent-chat.spec.ts`）：Web stream 完成态与 history timeline 完成态折叠；running history refresh 保持当前 turn 展开并顶部 `已处理 <duration>` 更新；640px 视口无横向溢出且保留左右 padding；滚动按钮淡入位置正确；running 时 `New Chat` 可点击；Markdown + `content_parts` 图片浮窗左右/键盘/遮罩/scrollIntoView；`AI Agent Chat marks runner-call finished failures as failed`；外部 runner terminal failed 状态 Error 展示。
- 真实接口验证：
  - `codex`：`POST /_bifrost/api/im-gateway/chat/stream` 返回 `run_started -> run_finished(status=failed)`；session projection `run_state=failed`；失败原因来自本机 `service_tier=default`。
  - `traex`：stream + `POST /chat/runs/{runId}/stop`；stream `run_started -> status -> run_finished(status=stopped)`；run detail `External CLI run was stopped by request.`；session projection failed；无残留。
  - `abc/chatgpt_web`：stream + stop；`run_started -> run_failed -> run_finished(status=stopped)`；run detail `ChatGPT Web run was stopped by request.`；无 Edge 残留。

### 真实场景测试（human_tests）

- 更新 `human_tests/im-gateway-agent.md`：IM→Web timeline 可见；Web 刷新过程完整；External Runner from IM→Web normalized progress；IM 触发 conversation 后 Web 发消息飞书 card 同步；Web 触发 conversation 后 IM 通道看到状态卡/最终消息；内置 vs. External Runner running/failed/completed 双视图一致。
- 更新 `human_tests/agent-session-persistence.md`：陈旧 Running 状态修复与 Stop 一致性；完成态折叠回归；running turn 刷新 placeholder。

启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent conversation timeline run_state`
- `cargo test -p bifrost-admin im_gateway agent_chat sessions_all sessions_history`
- `cd web && pnpm exec playwright test tests/ui/agent-chat.spec.ts`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 生效时不跑 `make coverage`；交付时说明。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：canonical timeline 单一事实源；`/sessions/all` 合并；Web 消费 history events；running turn placeholder；ChannelBinding fan-out；统一 `AgentRunState`；External Runner 归一化；折叠与图片浮窗；stop 语义。
- 复核 diff：agent/admin/im_gateway/external_cli/web/playwright/human_tests。
- 重点：live vs. persisted 优先级；stop 修复陈旧 running 并暴露修复计数；runner-call failure；Codex service_tier；ChatGPT Web stop 清理 Edge；binding cursor 幂等。
- 运行受影响单元 + E2E + Playwright。

### 第 2 轮

- 复核第 1 轮问题修复与 human_tests 索引。
- 复查跨通道 fan-out 幂等性；binding degraded 状态与 Web/canonical timeline 隔离。
- 复查折叠展开态、暗色主题、深链、process block 交互。
- 再跑失败用例；追加轮次直到 running/completed/failed 三视图一致、跨通道 fan-out 无重复。

## 风险与决策点

- **历史文件被 retention 清理**：active session 允许 `history_path` 为空；Web 退回 messages-only 并明确状态。
- **running turn JSONL 正在写**：读取 events 支持 append-only 部分可读；必要时 lossy loading。
- **External Runner 事件 schema 不统一**：先定义 canonical mapper，再接各 adapter。
- **同 session_key 多 runner 并存**：保留 `adapter/runner_id` 维度，避免串线。
- **Runner-call 子线程隔离**：`runner-call:*` 顶层列表隐藏，父消息链接到子 timeline。
- **多通道 fan-out 重复投递**：binding-level `last_delivered_event_id` / 幂等 key 控制 Web SSE / IM card。
- **IM card 被删除/过期/无权限更新**：binding 进入 degraded；Web 与 canonical timeline 不受影响。
- **Web 与 IM 同时发消息**：明确排队策略，禁止两个 turn 并行改同一 context。
- **不做旧数据兼容**：部分历史 session 只可展示不可回放或需重新索引，可接受。
- **`session_state.json` 语义收敛**：不再作事实源；老工具/脚本若仍读 `messages` 字段应视为 legacy。
- **跨装置 running 语义**：非内置 runner 保留 persisted running；避免误清导致跨装置视图丢状态。
- **Codex `service_tier` 默认注入**：与用户全局 Codex 配置存在交互；显式 override 保留是硬约束。
- **ChatGPT Web stop**：Edge managed profile 清理必须 profile 化，不能全局杀 Edge。
