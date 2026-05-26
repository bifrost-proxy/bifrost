# Chat Slash Runner Call 设计方案

## 功能模块说明

Agent Chat 输入框支持输入 `/` 后选择一个不同 Runner。选择后输入框展示 Runner 名称 chip，用户继续输入消息并发送。发送不会切换当前会话默认 Runner，而是发起一次用户主动触发的跨 Runner 调用：后端把当前会话上下文和用户消息打包给目标 Runner，目标 Runner 执行完成后，结果以一条 Runner Call 消息回到当前会话，并成为当前 Runner 后续对话可消费的上下文。

这类调用语义接近工具调用，但触发者是用户；调用结果既要可见，也要进入后续上下文。

## 用户目标验证清单

### 必须实现

- 在 Web Agent Chat 输入框中输入 `/` 时展示 Runner 选择面板。
- 选择 Runner 后在输入框中展示 Runner 名称 chip，chip 后可继续输入消息。
- 发送后调用目标 Runner，传入当前会话 user/assistant transcript 和本次用户消息。
- 目标 Runner 执行过程和最终结果展示在当前会话消息流中。
- 调用完成后，后续当前 Runner 的普通对话可以消费这次调用结果。

### 必须不破坏

- 普通输入发送、运行中 guide/queue、stop、线程切换和刷新恢复保持原行为。
- 选择 slash Runner 不改变当前会话默认 Runner。
- 外部 Runner 原有 `session_state`、conversation/thread resume 行为不被清空。
- 输入框在亮色和暗色主题下保持可读，不引入硬编码单主题颜色。

### 必须真实验证

- Web UI 用真实 Playwright 操作验证 `/` 选择 Runner、chip 展示、发送和结果渲染。
- API 用 mock 外部 Runner 验证 context bundle 包含当前会话 transcript。
- 后续普通发送的请求中包含 imported runner result。

## 数据模型

### Runner Call 请求

```json
{
  "callerSessionKey": "admin-chat-...",
  "callerRunnerId": "bifrost_agent",
  "callerRunnerAdapter": "bifrost_agent",
  "targetRunnerId": "codex",
  "message": "基于当前上下文给出实现建议",
  "workDir": "/Users/eden/work/github/bifrost",
  "historyPath": null,
  "callerMessages": [
    { "role": "user", "content": "..." },
    { "role": "assistant", "content": "..." }
  ]
}
```

### Imported Context

`session_state` 增加 `pendingImportedContexts`。每条记录包含：

- `callId`
- `sourceSessionKey`
- `targetRunnerId`
- `targetAdapter`
- `userMessage`
- `response`
- `createdAt`

外部 Runner 在 `build_prompt` 前消费 pending context，并追加到 instructions 中。内置 Bifrost Agent 在 `/agent/chat` 取出 session 后消费 pending context，并追加为上下文消息。

## API 设计

新增：

```http
POST /_bifrost/api/im-gateway/chat/runner-calls/stream
Content-Type: application/json
Accept: application/x-ndjson
```

返回 NDJSON：

- `runner_call_started`
- 目标 Runner 原始进度事件
- `runner_call_finished`
- `runner_call_failed`

## 上下文打包

后端构造 `RunnerContextBundle`：

```md
# Runner Call Context

Source session: ...
Current runner: ...
Target runner: ...

## Source Conversation Transcript

User:
...

Assistant:
...

## User Request For Target Runner

...
```

V1 使用 UI 当前展示的 `callerMessages` 作为 transcript 来源；这能覆盖 active/history/external session 已加载到页面后的真实上下文。后续可扩展为后端主动合并 active session detail 和 JSONL history。

## UI 设计

- 输入框输入 `/` 且没有选择 Runner 时展示 slash panel。
- slash panel 只列出与当前 Runner 不同的 Runner。
- 选择后显示 chip：`Run with <runner>`。
- 消息流中用户气泡显示 `Run with <runner>` chip 和用户输入。
- assistant 区域显示目标 Runner 的过程步骤和最终输出。
- 顶部当前 Runner tag 不变，表示当前会话默认 Runner 未切换。

## 测试方案

### 单元测试

- `session_state` 保存、读取、消费 imported context。
- Runner context bundle 渲染 transcript 和用户请求。
- Runner Call stream 请求缺少必要字段时返回 400。

### E2E/UI

- Playwright 输入 `/`，选择 `codex`，确认 chip 展示。
- 发送后断言请求到 `/chat/runner-calls/stream`，body 包含 `callerMessages`。
- mock stream 返回结果后，消息流展示 Runner Call 和最终输出。
- 再发一条普通外部 Runner 消息，断言请求 prompt/instructions 含 imported context。

### human_tests

更新 `human_tests/im-gateway-agent.md`：

- TC-IMA-126：Slash Runner Call 正常路径。
- TC-IMA-127：调用结果被下一轮当前 Runner 消费。
- TC-IMA-128：选择 Runner 不改变当前会话默认 Runner。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标、API、session_state 和 UI 交互。
- 执行 `git status --short`、`git diff`。
- 运行后端单元测试和 Playwright 定向用例。
- 修复发现的问题。

### 第 2 轮

- 复查第 1 轮修复后的 diff、human_tests 索引和上下文消费路径。
- 复跑受影响测试。
- 若仍发现功能缺口或测试失败，追加第 3 轮。

## 校验要求

- `cargo test -p bifrost-admin im_gateway::session_state`
- `cargo test -p bifrost-admin handlers::im_gateway::chat_gateway`
- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web run test:ui -- agent-chat.spec.ts`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
