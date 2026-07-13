# IM 引导模式和排队模式 真实场景测试用例

## 功能模块说明

IM Gateway 消息处理的两种模式，用于处理 Agent 正在处理中（session busy）时用户发来的新消息：

- **引导模式（内置 Bifrost Agent 默认）**：用户直接发送消息，消息被注入到 guide channel 中，在当前工具调用批次结束后追加到对话历史，进入下一个模型循环。多条尚未进入 loop 的引导消息会按到达顺序保留，并在消费时合并成一条 user message。
- **排队模式**（`/q <消息>` 或 ChatGPT Web 默认）：消息加入 FIFO 队列，每个 turn/run 完成后按顺序处理队列消息。最多排队 10 条。Codex/Traex app-server 与其他非 ChatGPT Web Runner 在 busy 时默认先尝试 live guide；不支持、拒绝或超时才明确降级排队并保留原消息和附件。
- **删除排队**（`/rq <序号>`）：通过序号删除指定的排队消息。
- **Codex Runner 接续**：Codex app-server 使用 `turn/steer` 追加当前 turn 引导；显式 `/q` 或 Guide 降级后的消息在 queue drain 时复用上一轮 `threadId` 接续同一个 Codex session。

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
- **预期结果**: 所有 15 个测试用例通过：
  - `test_guide_inject_appends` — 引导消息按顺序累积
  - `test_guide_status_is_readonly` — 引导状态查询不影响待消费消息
  - `test_guide_status_includes_worker_handoff_snapshot` — 已转交隔离 worker 的 guide 在 turn 完成前仍可被 `/status` 观测
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
  4. 非 ChatGPT Web Runner 默认 — 请求 active worker Guide，失败后调用 `queue_manager.push_queue()`；ChatGPT Web 直接排队等待当前 run 结束

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
  - 普通消息被事件循环转交给隔离 worker 后，`/status` 仍能看到 `pending_guide_messages: ["默认引导消息"]`，直到当前 turn 完成并清理 session queue 状态。
  - `/q <消息>` 仍显式进入 queue，不受默认 guide 策略影响。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin busy_default_mode --lib`、`cargo test -p bifrost-admin apply_busy_message_default --lib` 和 `BIFROST_PORT=18897 MOCK_PORT=18898 bash e2e-tests/tests/test_im_guide_queue_human_api.sh`。E2E 通过 `/_bifrost/api/im-gateway/debug/mock-inbound` 注入真实 IM inbound 事件，在内置 Bifrost Agent active turn 期间发送普通消息，`/agent/chat` 的 `/status` 返回 pending guide `["默认引导消息"]`，最终 mock 模型请求也收到该 guide。
- **回归执行记录（2026-06-02）**: PASS — CI run `26798673764` 的 macOS aarch64 shell shard 暴露隔离 worker handoff 后 `/status` 返回 `pending_guide_messages: []`。修复后执行 `source ~/.zshrc && SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin im_gateway::queue_manager::tests::test_guide_status_includes_worker_handoff_snapshot` 通过，验证 handed-off guide 快照清理边界；执行 `source ~/.zshrc && SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_E2E_HTTP_RETRIES=2 bash e2e-tests/tests/test_im_guide_queue_human_api.sh` 通过，验证真实 Bifrost + mock inbound 链路中默认 IM 消息仍作为 pending guide 展示并被 worker 消费。
- **回归执行记录（2026-06-02）**: PASS — CI run `26813811064` 的 macOS aarch64 shell shard 3 暴露内置 IM Agent active turn 阻塞在隔离 worker/mock model 时，主进程只等待 worker event，没有监听 guide channel notification，导致 busy 普通 IM 消息已经进入 pending guide 但未及时转发给 worker，脚本失败于 `default IM inbound guide was not consumed by the active loop`。修复后 `process_agent_chat()` 在 worker wait loop 中同时监听 guide notification，收到 guide 后立即 drain、记录 handed-off snapshot 并发送 worker `Guide` 命令；执行 `source ~/.zshrc && SKIP_FRONTEND_BUILD=1 BIFROST_E2E_HTTP_RETRIES=2 bash e2e-tests/tests/test_im_guide_queue_human_api.sh` 通过，验证真实 Bifrost + mock inbound 链路中默认 IM 消息会被及时转发并被 active loop 消费。

### TC-GQ-16: 外部 Runner busy 默认 Guide，ChatGPT Web 与失败路径进入 queue

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin busy_default_mode_guides_external_runners_except_chatgpt_web --lib
  cargo test -p bifrost-admin apply_busy_message_default_queues_custom_runner_messages --lib
  cargo test -p bifrost-admin codex_runner_metadata_resumes_queued_messages_after_current_run --lib
  cargo test -p bifrost-admin codex_runner_metadata_does_not_override_explicit_thread --lib
  SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_external_runner_live_guide.sh
  ```
- **预期结果**:
  - `runner = "codex"`、`"traex"`、`"claude_code"` 或其他非 ChatGPT Web runner 时 busy 默认策略为 ExternalGuide；ChatGPT Web 为 Queue。
  - 普通消息与 `/g <消息>` 尝试注入 active worker；不支持、拒绝或超时则完整进入 FIFO queue，`/q` 始终直接排队。
  - Codex/Traex app-server 接收 `turn/steer`，并在等待 ACK 时保持 runner control future 持续运行。
  - 上一轮 Codex result metadata 中的 `threadId` 会注入下一条排队消息的 request params；显式传入的 `threadId` 不会被覆盖。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin busy_default_mode --lib`、`cargo test -p bifrost-admin apply_busy_message_default --lib`、`cargo test -p bifrost-admin codex_runner_metadata --lib`、`codex exec --help` 和 `codex exec resume --help`。本机 Codex CLI `0.132.0` 显示 `exec` 只接收初始 prompt/stdin，`resume` 支持按 session/thread 接续下一轮；未发现运行中追加 guide 的 CLI 命令。

### TC-GQ-17: Web Agent Chat `/q` 竞态不会写入普通对话消息

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin queue_control_stream_input --lib
  ```
- **预期结果**:
  - `/q <消息>` 在 Web stream 入口被识别为队列控制命令，返回 `queued: true`、`queueLength` 和 `queueItems`。
  - `/rq <序号>` 在 Web stream 入口被识别为删除排队命令，返回更新后的 queue snapshot。
  - 普通消息不会被该控制命令 helper 截获。
  - 上述控制命令不启动新的 Agent turn，不会写入 JSONL `user_message`，因此不会在 Web 对话记录中显示为普通用户消息。
- **执行记录（2026-06-16）**: PASS — 执行 `cargo test -p bifrost-admin queue_control_stream_input --lib`，2 个回归测试通过，覆盖 `/q` 入队和 `/rq` 删除排队项。

### TC-GQ-18: Web Agent Chat active detail 的 idle 真源覆盖旧 running timeline

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin session_detail_without_active_status_reports_explicit_idle_state --lib
  pnpm test:unit AgentChatSection.timeline.test.ts
  pnpm test:ui --grep "active detail idle run_state"
  ```
- **预期结果**:
  - 后端 `GET /agent/sessions/:key` 在没有 active status 时显式返回 `running:false`、`state:"idle"` 和 `run_state:"idle"`。
  - 前端 timeline replay 遇到 live summary/detail 明确 idle 时，不用历史 `run_state_changed: running` 驱动当前运行态。
  - 刷新 `view=active` 的 Web Agent Chat 页面时状态标签显示 Ready，不显示 Stop，不追加 `Agent is running...` 占位消息。
- **执行记录（2026-06-16）**: PARTIAL — 执行 `cargo test -p bifrost-admin session_detail_without_active_status_reports_explicit_idle_state --lib` 通过，验证后端 detail idle 真源。执行 `pnpm test:ui --grep "active detail idle run_state"` 通过，新增 Playwright 用例覆盖 active detail `run_state:"idle"` + stale running history 的页面期望。执行 `pnpm test:unit AgentChatSection.timeline.test.ts` 在 Vitest worker 启动阶段失败，错误为 `ERR_REQUIRE_ESM`（`html-encoding-sniffer` require ESM `@exodus/bytes`），未进入新增断言，待本地 Vitest/jsdom 依赖环境修复后复跑。

### TC-GQ-19: 隔离 Worker 引导消息 IPC 竞态不丢失（确认应答 + 重新入队）

- **背景**: 隔离 worker 子进程下，引导消息经过两段异步跳转（父进程 `forward_pending_guides_to_worker` → worker stdin 的独立 `std::thread` 读取线程 `push_back`）。在 CPU 高竞争（CI 上 `BIFROST_E2E_SHELL_JOBS=4` 并发 4 个重脚本）时，`push_back` 可能晚于 worker 的 turn-end 单次非阻塞 drain，引导消息被静默丢弃，不触发第二次模型调用。表现为 `test_im_guide_queue_human_api.sh` 的 DRAIN 断言失败（"default IM inbound guide was not consumed by the active loop"）。
- **根因**: worker 的 turn-end drain 是单次非阻塞检查，与跨 IPC 管道的 `push_back` 存在竞态；父进程原有的 post-turn 重新入队只检查父侧 `guide_channel`，但该引导已被 `mark_guides_handed_to_worker` 移入 `handed_off_guides`，无法被回收。
- **修复**: 基于确认应答的重新入队。worker 记录实际消费的引导 `consumed_guide_messages`，通过 `AgentWorkerRunResult` 回传父进程；父进程用 `reconcile_handed_off_guides` 将 handed-off 集合与已消费集合对账，把"已交付但未消费"的引导重新 `push_queue`，由下一轮处理。
- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin im_gateway
  cargo test -p bifrost-agent session
  ```
- **预期结果**:
  - `reconcile_handed_off_guides` 三个单测通过：未消费引导被返回（`returns_unconsumed`）、全部消费返回空（`all_consumed`）、未交付返回空（`none_handed`）。
  - `bifrost-admin` im_gateway 全量测试通过（447 passed, 0 failed）。
  - `bifrost-agent` session 全量测试通过（130 passed, 0 failed），覆盖 turn-end / mid-turn 两处 `consumed_guide_messages` 记录。
  - 真实并发复现：修复前同等 4-way 并发稳定复现 DRAIN 失败 2/24；修复后在干净端口下的多轮并发运行 0 次 DRAIN 失败。
- **执行记录（2026-06-16）**: PASS — `cargo fmt --all -- --check` 通过；`cargo clippy -p bifrost-agent -p bifrost-admin --all-targets -- -D warnings` 0 warning；`cargo test -p bifrost-admin im_gateway` 447 passed / 0 failed；`cargo test -p bifrost-agent session` 130 passed / 0 failed。真实复现对照：修复前 4-way 并发稳定复现 DRAIN 丢失（保留的 mock 日志证明引导"丢失非延迟"，第二轮请求从未到达 mock）；修复后干净端口并发运行未再出现任何 DRAIN 失败（残留失败均为端口占用 `Errno 48` 的环境噪声，已与丢引导 bug 区分）。

### TC-GQ-20: Claude Code busy 消息通过 stream-json 中断并接续当前 session

- **操作步骤**:
  ```bash
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
    mock_stream_json_runner_redirects_live_guide_in_same_process --lib -- --nocapture
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
    result_frame_force_kills_runner_that_does_not_exit --lib -- --nocapture
  SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
    bash e2e-tests/tests/test_external_runner_live_guide.sh
  ```
- **预期结果**:
  - Claude Code 默认命令包含 `--input-format stream-json --output-format stream-json --replay-user-messages`，显式 text/custom args/exec 保留 queue fallback。
  - 首条 prompt 与 busy guide 使用同一个 mock Claude 进程和 session；guide 先发送 `control_request/subtype=interrupt`，收到 request id 匹配的 success response 后才发送 user JSONL 帧，不启动第二个进程或额外 resume。
  - 只有 interrupt 成功且 replay user frame 回显确认 guide 后，CLI/API 才返回 `delivery=steered`；`threadId` 为 Claude `session_id`，`turnId` 为空，表示 session redirect 而不是 Codex 风格的 same-turn steer。
  - transport 抑制已确认 interrupt 对应的 `result/error_during_execution`，继续等待 guide 的最终 success；外部只看到一个成功的 `run_finished`，最终响应为 `GUIDED_claude`。
  - runner 已输出终态 result 但不退出时，grace timeout 后终止进程组，并在 5 秒测试上限内完成 stderr reader 清理，不残留 `ACTIVE_RUNS`。
  - Codex/Traex app-server steer、Codex app-server reject-to-queue、Claude interrupt control reject-to-queue、显式 exec queue fallback 和 inactive session reject 同时回归通过。
- **执行记录（2026-07-12）**: PASS — `mock_stream_json_runner_accepts_live_guide_in_same_process` 通过；重新构建当前源码后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh` 输出 `[external-runner-live-guide] PASS`。mock 记录证明 Claude 初始帧与 guide 帧 PID 相同，CLI 返回 `delivery=steered`、`threadId=thread-claude`、无 `turnId`，最终同一 run 返回 `GUIDED_claude`。
- **执行记录（2026-07-13）**: PASS — 先用本机 Claude Code `2.1.207` 真实验证官方控制协议：长工具执行期间发送 interrupt control request，收到 matching success response 后发送 guide；同一 PID 依次输出旧响应 `error_during_execution` 与新响应 `success/GUIDED`。随后 `mock_stream_json_runner_redirects_live_guide_in_same_process` 和 `result_frame_force_kills_runner_that_does_not_exit` 各 `1 passed`，后者在约 2.17 秒内结束忽略 SIGTERM 的 mock；重新构建后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh` 输出 `[external-runner-live-guide] PASS`，Claude 外部事件只有一个成功 `run_finished`；mock Claude interrupt 明确拒绝时返回 `delivery=queued`，首轮 `FIRST_claude-reject` 后仅执行一次 `QUEUED_claude-reject`，拒绝原因完整保留。

### TC-GQ-21: Claude stream-json 就绪后立即发送 live guide（macOS CI 回归）

- **操作步骤**:
  ```bash
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
    bash e2e-tests/tests/test_external_runner_live_guide.sh
  ```
- **预期结果**:
  - mock Claude 在输出 `system/init` 和回放首条 user frame 后记录 `stream_ready`；测试脚本以该事件而非 app-server 专用的 `turn_ready` 作为 Claude guide 的发起条件。
  - 同一 mock PID 收到一次 `control_request/interrupt` 与一次 guide user JSONL 帧；CLI 返回 `delivery=steered`，最终唯一 `run_finished` 为 `succeeded` 且响应 `GUIDED_claude`。
  - 在 macOS shell E2E 分片中不因无效的 10 秒等待挤压 30 秒 runner timeout。
- **执行记录（2026-07-13）**: PASS — 重新构建后执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh` 输出 `[external-runner-live-guide] PASS`。该命令覆盖 Claude stream-json 的 `stream_ready` 触发、同 PID interrupt-and-guide、Codex/Traex app-server guide、Web/IM guide、reject-to-queue 与显式 exec queue fallback。

## 清理步骤

```bash
# 停止测试服务
# Ctrl+C 或 kill 进程

# 清理测试数据目录
rm -rf ./.bifrost-test

# 清理测试 provider
curl -s -X DELETE http://127.0.0.1:8801/_bifrost/api/im-gateway/providers/test-p1
```
