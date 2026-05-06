# IM 引导模式和排队模式 真实场景测试用例

## 功能模块说明

IM Gateway 消息处理的两种模式，用于处理 Agent 正在处理中（session busy）时用户发来的新消息：

- **引导模式（默认）**：用户直接发送消息，消息被注入到 guide channel 中，在当前工具调用批次结束后追加到对话历史，进入下一个模型循环。新消息覆盖前一条未处理的引导消息（覆盖语义）。
- **排队模式**（`/q <消息>`）：消息加入 FIFO 队列，每个 turn 完成后按顺序处理队列消息。最多排队 10 条。
- **删除排队**（`/rq <序号>`）：通过序号删除指定的排队消息。

核心组件：
- `SessionQueueManager`：管理引导通道和排队队列
- `AgentSession.guide_channel`：mid-turn 注入共享通道
- `run_agent_chat_with_interleave`：使用 `tokio::select!` 交错处理事件
- `handle_busy_message`：路由 `/q`、`/rq` 和默认引导模式

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
- **预期结果**: 所有 11 个测试用例通过：
  - `test_guide_inject_overwrite` — 引导消息覆盖语义
  - `test_queue_push_pop` — 队列 FIFO 推入弹出
  - `test_queue_remove` — 按序号删除队列消息
  - `test_queue_max_size` — 队列满（10 条）时拒绝新消息
  - `test_clear_session` — 清理 session 同时清除引导和队列
  - `test_guide_channel_producer_consumer_flow` — 生产者/消费者共享通道流
  - `test_guide_and_queue_coexistence` — 引导和队列独立共存
  - `test_queue_status_is_readonly` — 状态查询不影响队列
  - `test_session_isolation` — 不同 session 之间完全隔离
  - `test_remove_nonexistent_seq` — 删除不存在的序号返回 false
  - `test_concurrent_access` — 多线程并发读写安全

### TC-GQ-02: guide_channel 字段存在于 AgentSession

- **操作步骤**:
  ```bash
  grep -n 'guide_channel' crates/agent/src/session.rs
  ```
- **预期结果**: 找到以下关键位置：
  - 结构体字段声明 `pub guide_channel: Option<Arc<Mutex<Option<String>>>>`
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
  3. 默认 — 调用 `queue_manager.inject_guide()` 注入引导消息

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
  if let Some(ref guide) = session.guide_channel {
      let guide_msg = guide.lock().unwrap().take();
      if let Some(guide_msg) = guide_msg {
          session.add_user_message(&guide_msg);
          // 同时记录到 recorder
      }
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

## 清理步骤

```bash
# 停止测试服务
# Ctrl+C 或 kill 进程

# 清理测试数据目录
rm -rf ./.bifrost-test

# 清理测试 provider
curl -s -X DELETE http://127.0.0.1:8801/_bifrost/api/im-gateway/providers/test-p1
```
