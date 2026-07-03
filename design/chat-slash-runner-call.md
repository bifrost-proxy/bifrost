# Agent Chat Slash Runner Call 设计方案

## 背景

Bifrost Web 端 Agent Chat 只能与当前会话默认 Runner 单点对话；用户想临时借用另一个 Runner（例如 Codex 会话中借 ChatGPT Web / Bifrost Agent）时，必须切换 provider/channel 或另开线程，导致上下文断裂。

本方案引入 slash runner call：在输入框输入 `/` 后展示 Runner 选择面板；选中后输入框展示 `Run with <runner>` chip，用户继续输入消息并发送。发送不切换当前会话默认 Runner，而是发起一次用户显式触发的 Runner 调用：后端把当前会话上下文和本次消息打包给目标 Runner，目标 Runner 执行完成后，结果以一条 Runner Call 消息回到当前会话，并成为当前 Runner 后续对话可消费的上下文。

调用语义近似工具调用，但触发者是用户；调用结果既要可见，也要进入后续上下文。

## 用户目标验证清单

### 必须实现

- 在 Web Agent Chat 输入框中输入 `/` 时展示 Runner 选择面板（包含内置 Bifrost Agent 与所有外部 Runner）。
- 选择 Runner 后输入框展示 `Run with <runner>` chip，chip 后可继续输入消息。
- 发送后调用目标 Runner，请求体包含当前会话 user/assistant transcript 与本次用户消息。
- 目标 Runner 的执行过程与最终结果展示在当前会话消息流中。
- 调用完成后，后续当前 Runner 的普通对话可以消费这次调用结果。
- 页面刷新后，从源会话 detail 恢复 `Run with <runner>` 用户消息与 running/完成状态。

### 必须不破坏

- 普通输入发送、运行中 guide/queue、stop、线程切换、刷新恢复保持原行为。
- 选择 slash Runner 不改变当前会话默认 Runner；顶部 Runner tag 仍显示默认 Runner。
- 外部 Runner 原有 `session_state`、conversation/thread resume 行为不被清空。
- `runner-call:*` 子会话不会作为新线程出现在 Agent Chat 线程列表。
- 输入框在亮色和暗色主题下保持可读，不引入硬编码单主题颜色。

### 必须真实验证

- Web UI 用真实 Playwright 操作验证 `/` 触发 Runner 选择、chip 展示、发送与结果渲染。
- API 用 mock 外部 Runner 验证 context bundle 包含当前会话 transcript，并验证内置 Bifrost Agent 可作为目标 Runner 被调用。
- 再发一条普通外部 Runner 消息，断言请求 prompt/instructions 含 imported context。
- 断线重连或页面刷新后源会话内 `Run with <runner>` 与最终结果都能恢复。

## 产品语义

### Slash Runner Call vs 切换默认 Runner

- Slash Runner Call：一次用户触发的临时 Runner 调用，目标 Runner 处理完就返回，不改变会话默认 Runner。
- 切换默认 Runner：修改 Provider Agent 配置或选择另一个 provider 会话，改变后续所有消息的默认 Runner。

Chip 视觉与消息气泡颜色都必须让用户能看出这是一次借用而不是切换。

### 内置 Bifrost Agent 与外部 Runner 的区别

- 外部 Runner（`codex / custom / mock / chatgpt_web` 等）：通过 chat gateway `/runner-calls/stream` 打进 external runner 执行链路，`callerMessages` 直接进入 prompt bundle。
- 内置 Bifrost Agent：走独立 `runner-call:<source>:bifrost_agent` 子会话执行，通过 `/agent/chat` 入口。子会话仅作为内部执行容器，不出现在 Agent Chat 线程列表。

### 结果消费

调用完成后，源会话的 `session_state.pending_imported_contexts` 追加一条 `ImImportedRunnerContext`（`call_id / source_session_key / target_runner_id / target_adapter / user_message / response / created_at`）。当前 Runner 下一次消息发送前会消费该 context，并追加到 instructions 中，让后续对话可以引用这次调用结果。取出后立即消费一次，避免重复注入。

## 数据模型

### Runner Call 请求

~~~json
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
~~~

### Session Imported Context

`session_state.rs` 中已存在字段与工具函数：

~~~rust
pub struct SessionState {
    pub pending_imported_contexts: Vec<ImImportedRunnerContext>,
    // ...
}

pub struct ImImportedRunnerContext {
    pub call_id: String,
    pub source_session_key: String,
    pub target_runner_id: String,
    pub target_adapter: String,
    pub user_message: String,
    pub response: String,
    pub created_at: String,
}
~~~

工具函数：`push_imported_context`、`take_imported_contexts`、`render_imported_contexts`；调用者去重与幂等由 `call_id` 保证。

### Runner Call 生命周期

后端在收到 stream 请求后必须立即持久化到源会话：

- 用户气泡：`Run with <runner>` + 用户消息文本。
- Assistant 状态：`Runner <runner> is running...`（含 spinner）。

`runner-call:*` 子会话仅作为内部执行容器；不作为新线程展示。目标 Runner 完成后，源会话内 running assistant 消息原地更新为最终结果，`pending_imported_contexts` 追加对应记录。刷新页面时前端从源会话 detail 恢复 running/finished 状态，不依赖 stream 未接收部分。

## API 设计

新增：

~~~http
POST /_bifrost/api/im-gateway/chat/runner-calls/stream
Content-Type: application/json
Accept: application/x-ndjson
~~~

已实现路径：`crates/bifrost-admin/src/handlers/im_gateway/chat_gateway.rs` 中 `/runner-calls/stream` 分支，包含 `RunnerCallStreamRequest` / `RunnerCallMessage` / `RunnerCallTarget::{BuiltinAgent, External}` / `resolve_runner_call_target` / `runner_call_stream_response` / `builtin_runner_call_stream_response`。

响应 NDJSON 事件：

- `runner_call_started`：`callId / sourceSessionKey / targetRunnerId / targetAdapter`
- 目标 Runner 原生进度事件：`assistant_delta / tool_finished / plan_step_delta` 等
- `runner_call_finished`：`callId / response / durationMs`
- `runner_call_failed`：`callId / error`

请求校验：`targetRunnerId` 缺失或未知返回 400；`callerSessionKey` 或 `message` 为空返回 400。

## 上下文打包

后端构造 `RunnerContextBundle`（Markdown 形式），作为目标 Runner 的 prompt/instructions 前置块。核心结构：

- 顶部 metadata：`Source session / Current runner / Target runner`。
- `## Source Conversation Transcript`：按时间顺序列出 `callerMessages` 中的 user/assistant 内容。
- `## User Request For Target Runner`：本次 `message` 全文。

V1 使用 UI 当前展示的 `callerMessages` 作为 transcript 来源；这能覆盖 active/history/external session 已加载到页面后的真实上下文。后续可扩展为后端主动合并 active session detail 与 JSONL history。

## UI 设计

- 输入框输入 `/` 且没有选择 Runner 时展示 slash panel。
- slash panel 列出全部可用 Runner（包含当前 Runner 与内置 Bifrost Agent）。
- 选择后显示 chip：`Run with <runner>`；chip 有独立 close 按钮，允许在发送前撤销选择。
- 消息流用户气泡显示 `Run with <runner>` chip + 用户输入。
- Assistant 区域显示目标 Runner 的过程步骤与最终输出，视觉与普通 assistant 消息一致，但顶部有 `Runner <name>` 徽标。
- 顶部当前 Runner tag 不变，表示当前会话默认 Runner 未切换。
- 刷新页面后，源会话仍能恢复已持久化的 Runner Call 用户消息与 running/完成状态；`runner-call:*` 子会话不作为新线程展示。

实现文件：

- `web/src/pages/AI/AgentChatSection.runnerCall.tsx`：`useSlashRunnerPanel`、`useRunnerCallHandler`、`RunnerCallChip` 等 Hook 与组件。
- `web/src/pages/AI/AgentChatSection.tsx`：输入框事件监听、slash panel 展示与选择处理。
- `web/src/api/agent-chat.ts`：`runRunnerCallStream` 客户端 SSE 消费。

## CLI

无 CLI 变更。

## Admin API 与 Sync 边界

- Admin API 唯一新增点是 `/chat/runner-calls/stream`；其他 chat/runs API 不变。
- Sync 不涉及 Runner Call：`runner-call:*` 子会话 session_key 命名固定，Sync 白名单默认过滤此前缀；源会话的 `pending_imported_contexts` 是本机 runtime 状态，不参与 remote sync。

## 实现切分

### Phase 1：后端 stream

- Chat Gateway 新增 `/runner-calls/stream` 分支。
- `RunnerCallStreamRequest` / `RunnerCallTarget` / `resolve_runner_call_target` 完成。
- 内置 Bifrost Agent 子会话通过 `builtin_runner_call_stream_response` 承接。
- 外部 Runner 复用 `ExternalCliRunRequest` 派发。

### Phase 2：Session 与 Imported Context

- `session_state.rs` 中 `pending_imported_contexts` / `push_imported_context` / `take_imported_contexts` / `render_imported_contexts` 完成。
- 外部 Runner 在 `build_prompt` 前消费 pending context 并追加到 instructions。
- 内置 Bifrost Agent 在 `/agent/chat` 取出 session 后消费。

### Phase 3：Web UI

- `useSlashRunnerPanel` 监听 `/` 输入触发、rune 过滤、方向键选择。
- `RunnerCallChip` 展示与撤销。
- 发送时改用 `runRunnerCallStream`；SSE 事件转成消息流事件。
- 刷新恢复：`fetchSessionDetail` 结果里包含 running assistant 状态；前端根据 `runner_call_started/finished/failed` 语义补齐。

### Phase 4：测试与文档

- 后端单元测试 + Playwright + human_tests 更新。

## 测试方案

### 单元测试

- `session_state::imported_contexts_are_pushed_rendered_and_consumed_once`（已存在）。
- `runner_call_target_resolution_prefers_builtin_when_id_matches`。
- `runner_call_stream_returns_400_when_missing_target_runner`。
- `runner_call_stream_persists_run_with_and_running_assistant_in_source_session`。
- `runner_call_stream_finished_updates_running_assistant_in_place`。

### E2E / UI

- Playwright 输入 `/`，选择 `codex`，确认 chip 展示。
- 发送后断言请求打到 `/chat/runner-calls/stream`，body 含 `callerMessages`。
- Mock stream 返回结果后，消息流展示 Runner Call 用户气泡与最终输出。
- Mock 线程列表返回 `runner-call:*` 子会话时，UI 不展示子线程，并保留源会话中的 running 状态。
- 再发一条普通外部 Runner 消息，断言请求 prompt/instructions 含 imported context。

### human_tests

更新 `human_tests/im-gateway-agent.md`：

- TC-IMA-126：Slash Runner Call 正常路径（含刷新页面后从源会话持久化恢复 `Run with <runner>` 用户消息与目标 Runner running/完成状态、`runner-call:*` 子会话不展示为新线程的回归记录）。
- TC-IMA-127：调用结果被下一轮当前 Runner 消费。
- TC-IMA-128：选择 Runner 不改变当前会话默认 Runner。
- TC-IMA-145：Slash Runner Call 失败状态不误报成功；`runner_call_failed` 事件驱动 assistant 消息由 running 状态更新为失败状态并保留错误信息。
- TC-IMA-146（新增）：内置 Bifrost Agent 作为目标 Runner 的完整路径。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin im_gateway::session_state`
- `cargo test -p bifrost-admin handlers::im_gateway::chat_gateway`
- `pnpm --dir web exec tsc --noEmit`
- `pnpm --dir web run test:ui -- agent-chat.spec.ts`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定生效，`make coverage` 交由远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标、API、session_state 消费路径与 UI 交互。
- 执行 `git status --short`、`git diff`。
- 跑后端单元测试和 Playwright 定向用例；发现问题立即修复。

### 第 2 轮

- 复查第 1 轮修复后的 diff、human_tests 索引和上下文消费路径。
- 复跑受影响测试；重点复核 `runner_call_failed` 场景与刷新恢复路径。
- 若仍发现功能缺口或测试失败，追加第 3 轮。

## 风险与决策

- V1 直接把前端 `callerMessages` 作为 transcript 来源；如果前端未加载完整历史，目标 Runner 拿到的 transcript 可能不完整。后续可扩展后端从 active session detail + JSONL history 主动合并，本方案已保留 `RunnerContextBundle` 抽象层。
- `pending_imported_contexts` 只在下一次消息前被消费；若用户连续多次 slash 调用而不发送普通消息，可能堆积多条 context。控制策略：单次消费全部 pending contexts，并按时间顺序渲染。
- 子会话 session_key 命名固定 `runner-call:<source>:<target>`，需确保 Sync 白名单过滤此前缀，避免误上传。
- 内置 Bifrost Agent 作为目标 Runner 时使用独立 sub-session 执行；若用户在源会话与子会话之间频繁切换，需要保证 Web UI 不误把子会话展示为新线程。
- 失败路径必须显式区分 `runner_call_failed` 与目标 Runner 内部工具失败：前者直接把源会话 running assistant 标为失败；后者仍由目标 Runner 自身报告并写入结果消息。
