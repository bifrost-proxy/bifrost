# IM 引导模式和排队模式 真实场景测试用例

## 功能模块说明

IM Gateway 消息处理的两种模式，用于处理 Agent 正在处理中（session busy）时用户发来的新消息：

- **引导模式（内置 Bifrost Agent 默认）**：用户直接发送消息，消息被注入到 guide channel 中，在当前工具调用批次结束后追加到对话历史，进入下一个模型循环。多条尚未进入 loop 的引导消息会按到达顺序保留，并在消费时合并成一条 user message。
- **排队模式**（`/q <消息>` 或自定义 Runner 默认）：消息加入 FIFO 队列，每个 turn/run 完成后按顺序处理队列消息。最多排队 10 条。ChatGPT Web、Codex 和其他自定义 Runner 在 busy 时默认走排队，因为运行中没有内置 Agent 的 guide checkpoint；这些 Runner 收到 `/g <消息>` 时也会明确降级为排队消息。
- **删除排队**（`/rq <序号>`）：通过序号删除指定的排队消息。
- **Codex Runner 接续**：当前 Codex CLI 支持 `codex exec resume <thread_id> [PROMPT]` 做下一轮接续，但不支持运行中追加 guide；因此 busy 期间仍排队，队列 drain 时复用上一轮 `threadId` 接续同一个 Codex session。

核心组件：
- `SessionQueueManager`：管理引导通道和排队队列
- `AgentSession.guide_channel`：mid-turn 注入共享通道
- `run_agent_chat_with_interleave`：使用 `tokio::select!` 交错处理事件
- `handle_busy_message`：路由 `/q`、`/rq`，并按 runner 能力选择默认引导或默认排队

## 前置条件

```bash
# 启动 Bifrost 测试实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
```

确保 Agent 已启用（默认启用）并配置了有效的 model provider。

## 测试用例列表

### TC-GQ-01: SessionQueueManager 单元测试全部通过

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin -- queue_manager
  ```
- **预期结果**: 所有 14 个测试用例通过：
  - `test_guide_inject_appends` — 引导消息按顺序累积
  - `test_guide_status_is_readonly` — 引导状态查询不影响待消费消息
  - `test_queue_push_pop` — 队列 FIFO 推入弹出
  - `test_queue_remove` — 按序号删除队列消息
  - `test_queue_max_size` — 队列满（10 条）时拒绝新消息
  - `test_clear_session` — 清理 session 同时清除引导和队列
  - `test_guide_channel_producer_consumer_flow` — 生产者/消费者共享通道流
  - `test_guide_and_queue_coexistence` — 引导和队列独立共存
  - `test_queue_status_is_readonly` — 队列状态查询不影响队列
  - `test_session_isolation` — 不同 session 之间完全隔离
  - `test_remove_nonexistent_seq` — 删除不存在的序号返回 false
  - `test_concurrent_access` — 多线程并发读写安全

### TC-GQ-02: guide_channel 字段存在于 AgentSession

- **操作步骤**:
  ```bash
  grep -n 'guide_channel' crates/agent/src/session.rs
  ```
- **预期结果**: 找到以下关键位置：
  - 结构体字段声明 `pub guide_channel: Option<GuideChannel>`
  - 构造函数初始化 `guide_channel: None`
  - `run_turn_with_mcp` 中的 mid-turn 注入逻辑（检查 guide_channel 并追加到 history）

### TC-GQ-03: 服务启动成功，ImGatewayService 包含 queue_manager

- **操作步骤**:
  ```bash
  BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
  ```
- **预期结果**: 服务正常启动，无 panic 或编译错误。Admin UI 可访问 `http://127.0.0.1:8801/_bifrost/`

### TC-GQ-04: IM Gateway API 正常工作

- **操作步骤**:
  ```bash
  # 获取 agent 配置
  curl -s http://127.0.0.1:8801/_bifrost/api/im-gateway/agent | python3 -m json.tool

  # 获取 provider 列表
  curl -s http://127.0.0.1:8801/_bifrost/api/im-gateway/providers | python3 -m json.tool

  # 获取 session 列表
  curl -s http://127.0.0.1:8801/_bifrost/api/im-gateway/agent/sessions | python3 -m json.tool
  ```
- **预期结果**: 
  - Agent 配置返回 JSON 包含 `enabled: true`
  - Provider 列表返回数组（可能为空）
  - Session 列表返回 `{"sessions": []}`

### TC-GQ-05: handle_busy_message 路由逻辑验证（代码审查）

- **操作步骤**: 审查 `crates/bifrost-admin/src/handlers/im_gateway.rs` 中 `handle_busy_message` 函数
- **预期结果**: 函数包含三个分支：
  1. `/q <text>` — 调用 `queue_manager.push_queue()` 并回复队列状态
  2. `/rq <N>` — 调用 `queue_manager.remove_queue()` 并回复更新后的队列状态
  3. 内置 Bifrost Agent 默认 — 调用 `queue_manager.inject_guide()` 注入引导消息
  4. ChatGPT Web / Codex / 自定义 Runner 默认 — 调用 `queue_manager.push_queue()` 排队等待当前 run 结束

### TC-GQ-06: run_agent_chat_with_interleave 使用 tokio::select! 交错处理

- **操作步骤**: 审查 `run_agent_chat_with_interleave` 函数
- **预期结果**: 函数结构包含：
  - 外层 loop 用于 queue drain（处理初始消息后逐条处理队列）
  - 内层 `tokio::select!` 同时监听 `chat_future` 和 `rx.recv()`
  - chat 完成后调用 `queue_manager.pop_queue()` 继续处理队列
  - 队列为空时调用 `queue_manager.clear_session()` 并退出

### TC-GQ-07: handle_concurrent_event_during_chat 正确路由事件

- **操作步骤**: 审查 `handle_concurrent_event_during_chat` 函数
- **预期结果**: 函数包含：
  - owner 安全检查
  - session-free 命令快速通道（`/help` 等）
  - 当前活跃 session 的消息 → 走 `handle_busy_message`（guide/queue）
  - 不同 session 的消息 → 如果 session 也忙则走 `handle_busy_message`，否则排队

### TC-GQ-08: guide 注入在 mid-turn 工具调用批次后触发

- **操作步骤**: 审查 `crates/agent/src/session.rs` 中 `run_turn_with_mcp` 函数，定位 guide 注入代码
- **预期结果**: 注入逻辑位于工具调用结果处理后、mid-turn compaction 之前：
  ```rust
  let guide_messages = drain_guide_messages(session);
  if let Some(guide_msg) = combine_guide_messages(guide_messages) {
      session.add_user_message(&guide_msg);
      // 同时记录到 recorder
  }
  ```

### TC-GQ-09: 编译和全量测试通过

- **操作步骤**:
  ```bash
  cargo test --workspace --all-features
  ```
- **预期结果**: 所有测试通过，无编译错误

### TC-GQ-10: `/agent/chat` 注入 `guide_message` 时，turn-end guide 不丢失

- **操作步骤**:
  1. 启动真实 Bifrost 和 guide/queue 专用 mock provider（使用独立临时数据目录和 `--no-system-proxy`）。
  2. 调用：
     ```bash
     curl -s -X POST http://127.0.0.1:18897/_bifrost/api/im-gateway/agent/chat \
       -H 'Content-Type: application/json' \
       -d '{"session_key":"guide-end-test","message":"先处理 initial","guide_message":"这是 turn 结束前插入的 guide"}' | jq .
     ```
- **预期结果**:
  - `success: true`
  - 最终 `response` 明确体现 `guide_message` 已在同一次 turn loop 中继续处理
  - guide 文本不会被静默吞掉
- **执行记录（2026-05-05）**: PASS — 运行 `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`，`guide-end-test` 返回 `GUIDE_DRAINED: 先处理 initial -> 这是 turn 结束前插入的 guide`，确认 turn-end guide 已被同一次 turn loop 消费

### TC-GQ-11: `/agent/chat` 注入 `queue_messages` 时按 FIFO drain

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:18897/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"queue-test","message":"第一条","queue_messages":["第二条","第三条"]}' | jq .
  ```
- **预期结果**:
  - `success: true`
  - 最终 `response` 或 `tool_calls` 对应的 mock 顺序能够证明处理顺序为：`第一条 -> 第二条 -> 第三条`
  - queue 在同一次 `run_turn_with_mcp` 中被继续 drain，而不是只处理第一条
- **执行记录（2026-05-05）**: PASS — 同一脚本中 `queue-test` 返回 `ORDER: 第一条 -> 第二条 -> 第三条`，确认 `pending_messages` 按 FIFO 顺序连续处理

### TC-GQ-12: `guide_message` 优先于 `queue_messages`

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:18897/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"guide-priority-test","message":"初始消息","guide_message":"guide 插入","queue_messages":["queue-1","queue-2"]}' | jq .
  ```
- **预期结果**:
  - `success: true`
  - 最终处理顺序为：`初始消息 -> guide 插入 -> queue-1 -> queue-2`
  - guide 必须先于 queued messages 被消费
- **执行记录（2026-05-05）**: PASS — `guide-priority-test` 返回 `ORDER: 初始消息 -> guide 插入 -> queue-1 -> queue-2`，确认 turn-end guide 检查先于 pending queue drain 执行

### TC-GQ-13: 空白 `guide_message` 与空白 `queue_messages` 会被忽略

- **操作步骤**:
  ```bash
  curl -s -X POST http://127.0.0.1:18897/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"blank-ignore-test","message":"hello","guide_message":"   ","queue_messages":["","   ","real queued"]}' | jq .
  ```
- **预期结果**:
  - `success: true`
  - 只有 `real queued` 被继续处理；空白 guide 与空白 queue 项不会进入历史或 pending queue
  - 最终顺序体现为：`hello -> real queued`
- **执行记录（2026-05-05）**: PASS — `blank-ignore-test` 返回 `ORDER: hello -> real queued`，确认空白 guide / queue 项未参与后续 turn 处理

### TC-GQ-14: 多条尚未进入 loop 的 guide 合并并在 `/status` 展示明细

- **操作步骤**:
  ```bash
  SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" ADMIN_PORT=18131 MOCK_HTTP_PORT=18132 \
    bash e2e-tests/tests/test_im_guide_queue_human_api.sh
  ```
- **预期结果**:
  - 长模型请求运行期间，同 session 发送 `/status` 返回 `pending_guide_messages: ["第一条引导","第二条引导"]`
  - `/status.response` 包含 `引导消息: 2 条尚未进入 loop`，并列出两条具体引导内容
  - 原 chat 完成后，模型请求历史中只新增一条合并后的 user message，内容包含 `引导消息 1` 和 `引导消息 2`
  - 最终响应包含 `GUIDES_MERGED: 第一条引导 -> 第二条引导`
- **执行记录（2026-05-10）**: PASS — 执行 `source ~/.zshrc && SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" ADMIN_PORT=18131 MOCK_HTTP_PORT=18132 bash e2e-tests/tests/test_im_guide_queue_human_api.sh`，脚本启动临时 Bifrost 与 mock provider，通过 `/agent/chat` 注入两条 `guide_messages`，在首轮慢模型请求期间轮询 `/status` 并断言 pending guide JSON 与文案明细，随后断言最终模型请求收到合并后的单条 guide user message。
- **回归记录（2026-05-29）**: CI `26635014024` 暴露隔离 worker 场景中 `/status` 用主进程 guide queue 覆盖 worker active status，导致 `pending_guide_messages` 为空。修复后 `/status` 合并 worker active guides 与主进程 queue guides，并去重避免非隔离路径重复展示。

### TC-GQ-15: 内置 Bifrost Agent busy 普通 IM 消息默认进入 guide

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin busy_default_mode_is_guide_for_builtin_bifrost_agent --lib
  cargo test -p bifrost-admin apply_busy_message_default_guides_builtin_messages_without_queueing --lib
  ```
- **预期结果**:
  - `runner = null` 或 `runner = "bifrost_agent"` 时 busy 默认策略为 guide。
  - 普通消息进入 `guide_status`，不会进入 `queue_status`。
  - `/q <消息>` 仍显式进入 queue，不受默认 guide 策略影响。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin busy_default_mode --lib`、`cargo test -p bifrost-admin apply_busy_message_default --lib` 和 `BIFROST_PORT=18897 MOCK_PORT=18898 bash e2e-tests/tests/test_im_guide_queue_human_api.sh`。E2E 通过 `/_bifrost/api/im-gateway/debug/mock-inbound` 注入真实 IM inbound 事件，在内置 Bifrost Agent active turn 期间发送普通消息，`/agent/chat` 的 `/status` 返回 pending guide `["默认引导消息"]`，最终 mock 模型请求也收到该 guide。

### TC-GQ-16: 自定义 Runner busy 普通 IM 消息默认进入 queue，Codex 用 resume 接续

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin busy_default_mode_is_queue_for_custom_runner --lib
  cargo test -p bifrost-admin apply_busy_message_default_queues_custom_runner_messages --lib
  cargo test -p bifrost-admin codex_runner_metadata_resumes_queued_messages_after_current_run --lib
  cargo test -p bifrost-admin codex_runner_metadata_does_not_override_explicit_thread --lib
  codex exec --help | sed -n '1,120p'
  codex exec resume --help | sed -n '1,120p'
  ```
- **预期结果**:
  - `runner = "codex"`、`runner = "chatgpt-web"` 或其他自定义 runner 时 busy 默认策略为 queue。
  - 普通消息进入 FIFO queue，不进入 guide；`/g <消息>` 不会伪装成运行中 guide，而是明确作为 queue 处理。
  - Codex CLI help 只展示 `exec`/`resume` 的 prompt/stdin 接续能力，没有运行中追加 guide 的命令。
  - 上一轮 Codex result metadata 中的 `threadId` 会注入下一条排队消息的 request params；显式传入的 `threadId` 不会被覆盖。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin busy_default_mode --lib`、`cargo test -p bifrost-admin apply_busy_message_default --lib`、`cargo test -p bifrost-admin codex_runner_metadata --lib`、`codex exec --help` 和 `codex exec resume --help`。本机 Codex CLI `0.132.0` 显示 `exec` 只接收初始 prompt/stdin，`resume` 支持按 session/thread 接续下一轮；未发现运行中追加 guide 的 CLI 命令。

## 清理步骤

```bash
# 停止测试服务
# Ctrl+C 或 kill 进程

# 清理测试数据目录
rm -rf ./.bifrost-test

# 清理测试 provider
curl -s -X DELETE http://127.0.0.1:8801/_bifrost/api/im-gateway/providers/test-p1
```
