# External Runner 子 Agent 事件边界

## 背景

Codex / Trae X app-server 会在根 Agent 执行期间复用同一条 stdout JSON-RPC 流，输出根线程和子线程的事件。Bifrost 之前把所有 `turn/completed` 都解释为当前 IM 任务完成，并把 `collabAgentToolCall`、`subAgentActivity` 以及 Claude Code 的 `Task` / `Agent` 工具额外归一化成 `subagent_updated`。这会让子线程的完成、消息或工具细节越过根任务边界，造成任务提前退出，或让飞书进度卡片把子 Agent 内容渲染成根 Agent 结果。

## 目标

- 只有当前根 thread 与当前根 turn 的事件可以更新当前 IM 任务或结束任务。
- 根 Agent 发起的协作调用与其他工具调用一致：开始事件提供输入，完成事件提供输出和成功状态。
- 子线程内部的消息、推理、工具、状态和 `turn/completed` 不进入根任务事件流。
- Codex / Trae X 的 `subAgentActivity` 属于子线程内部遥测，不生成用户可见事件。
- Claude Code 的 `Task` / `Agent` 使用既有普通 `tool_started` / `tool_finished` 语义，不再生成专属子 Agent 事件。
- `/g`、`/stop`、容量重试和根 turn 的正常完成语义保持不变。

## 事件边界

### App-server

Bifrost 在 `thread/start|resume` 和 `turn/start` 的响应中保存根 `thread_id` 与当前 `turn_id`。收到通知时按以下规则过滤：

1. `item/*`、`turn/*`、`thread/*` 等作用域事件必须属于根 thread；事件携带 turn id 时还必须属于当前根 turn。
2. `turn/completed` 必须同时匹配根 thread 与根 turn，才可生成 `run_finished` / `run_failed` 并结束进程。
3. `account/rateLimits/updated` 等账户级事件没有 thread/turn 作用域，继续按原逻辑处理。
4. 根 turn 中的 `collabAgentToolCall` 仅产生普通工具开始/完成事件；`item/updated` 和 `subAgentActivity` 不产生事件。

协作工具输入只保留工具调用参数（例如 `prompt`、`receiverThreadIds`）；完成输出优先使用 provider 的 `result` / `error` / `message`。`agentsStates` 是子 Agent 内部生命周期状态，不展开成卡片步骤，也不用于结束根任务。

### Stream JSON

Codex / Trae X 的 `collabAgentToolCall` 仅在 `item.started` 与 `item.completed` 生成普通工具事件；`item.updated` 与 `subAgentActivity` 被忽略。Claude Code 的 `Task` / `Agent` 沿用通用工具解析逻辑。

## 兼容性

`subagent_updated` 数据类型及其历史回放渲染暂时保留，用于兼容升级前已经持久化的记录。新事件入口不再生成该类型。后续可在确认历史兼容窗口结束后单独清理模型和 UI 代码，本次修复不扩大到存量数据迁移。

## 验证

- 单元测试覆盖根/子 thread 与根/子 turn 的作用域矩阵。
- mock app-server 回归测试先发送子线程消息和 `turn/completed`，再发送根线程最终消息与完成事件，断言进程不会提前结束且子线程内容不进入输出。
- parser 测试断言 Codex / Trae X 协作调用和 Claude Code `Task` / `Agent` 都只生成普通工具输入/输出事件。
- E2E 通过真实 Bifrost Agent Run API 和 mock app-server 验证根结果、事件类型与退出状态。
- `human_tests/` 记录同一真实 CLI 链路的操作步骤和可核验断言。
