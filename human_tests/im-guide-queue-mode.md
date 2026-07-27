# IM 引导模式和排队模式 真实场景测试用例

## 功能模块说明

IM Gateway 消息处理的两种模式，用于处理 Agent 正在处理中（session busy）时用户发来的新消息：

- **排队模式**（`/q <消息>` 或 ChatGPT Web 默认）：消息加入 FIFO 队列，每个 turn/run 完成后按顺序处理队列消息。最多排队 10 条。Codex/Traex app-server 与其他非 ChatGPT Web Runner 在 busy 时默认先尝试 live guide；不支持、拒绝或超时才明确降级排队并保留原消息和附件。
- **删除排队**（`/rq <序号>`）：通过序号删除指定的排队消息。
- **Codex Runner 接续**：Codex app-server 使用 `turn/steer` 追加当前 turn 引导；显式 `/q` 或 Guide 降级后的消息在 queue drain 时复用上一轮 `threadId` 接续同一个 Codex session。

核心组件：
- `SessionQueueManager`：管理引导通道和排队队列
- external runner control channel：向支持 steer 的 active runner 注入引导
- `handle_busy_message`：路由 `/q`、`/rq`，并按 runner 能力选择默认引导或默认排队

## 前置条件

```bash
# 启动 Bifrost 测试实例
BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
```

确保已配置并启用一个外部 Runner。

## 测试用例列表

### TC-GQ-16: 外部 Runner busy 默认 Guide，系统 slash 不透传，失败路径进入 queue

- **操作步骤**:
  ```bash
  cargo test -p bifrost-admin busy_default_mode_guides_external_runners_except_chatgpt_web --lib
  cargo test -p bifrost-admin apply_busy_message_default_queues_custom_runner_messages --lib
  cargo test -p bifrost-admin codex_runner_metadata_resumes_queued_messages_after_current_run --lib
  cargo test -p bifrost-admin codex_runner_metadata_does_not_override_explicit_thread --lib
  SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_effort_slash --lib
  SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_external_runner_live_guide.sh
  ```
- **预期结果**:
  - `runner = "codex"`、`"traex"`、`"claude_code"` 或其他非 ChatGPT Web runner 时 busy 默认策略为 ExternalGuide；ChatGPT Web 为 Queue。
  - 普通消息尝试注入 active worker；隐藏兼容命令 `/g <消息>` 行为相同；不支持、拒绝或超时则完整进入 FIFO queue，`/q` 始终直接排队。
  - Codex/Traex active 时，`/efforts` 与 `/effort <级别>` 由 Bifrost 作为系统命令处理，不得进入 `turn/steer`；effort override 写入 session，并从下一轮生效。
  - Codex/Traex app-server 接收 `turn/steer`，并在等待 ACK 时保持 runner control future 持续运行。
  - 上一轮 Codex result metadata 中的 `threadId` 会注入下一条排队消息的 request params；显式传入的 `threadId` 不会被覆盖。
- **执行记录（2026-05-21）**: PASS — 执行 `cargo test -p bifrost-admin busy_default_mode --lib`、`cargo test -p bifrost-admin apply_busy_message_default --lib`、`cargo test -p bifrost-admin codex_runner_metadata --lib`、`codex exec --help` 和 `codex exec resume --help`。本机 Codex CLI `0.132.0` 显示 `exec` 只接收初始 prompt/stdin，`resume` 支持按 session/thread 接续下一轮；未发现运行中追加 guide 的 CLI 命令。
- **回归执行记录（2026-07-13）**: PASS — 执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin external_cli_effort_slash --lib -- --nocapture`（1 项）和 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_external_runner_live_guide.sh`。真实 Bifrost + mock IM inbound 链路分别保持 Codex/Traex active，发送 `/efforts`、`/effort high` 后确认两条系统命令均未进入 `turn/steer`，session effort override 为 `high` 且来源为 `session slash command`；随后普通消息仍作为唯一 Guide 注入并完成当前 turn。

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

### TC-GQ-19: 污染父环境启动 detached daemon 后仍可 live guide（macOS CI 回归）

- **操作步骤**:
  ```bash
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" \
    bash e2e-tests/tests/test_external_runner_live_guide.sh
  ```
- **预期结果**:
  - 测试脚本以 `BIFROST_EXTERNAL_CLI_WORKER=1` 的污染父环境执行 `bifrost start --daemon`；detached daemon 清除该内部 worker 标记，且测试只使用动态端口和带 ownership marker 的独立数据目录。
  - daemon 内 external runner 必须继续派生显式 worker，并在主进程注册 active worker session；Codex/Traex CLI、Web 和 mock Feishu IM 的 busy 消息均返回 `delivery=steered`，不得出现 `no active external runner` 后降级 Queue。
  - mock Claude 输出 `system/init` 和回放首条 user frame 后记录 `stream_ready`；测试以此而非 app-server 专用 `turn_ready` 触发 Claude/Claude-reject guide。
  - 同一 Claude mock PID 接收一次 interrupt 与一次 guide user JSONL 帧，最终唯一 `run_finished` 为成功的 `GUIDED_claude`；interrupt 拒绝则诚实降级 queue。
  - Codex/Traex app-server guide、Web/IM guide 与显式 exec queue fallback 同时通过。
- **执行记录（2026-07-13）**: PASS — 合并主分支后重新构建并执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh`，输出 `[external-runner-live-guide] PASS`。
- **回归执行记录（2026-07-27）**: PASS — 先执行 `SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost` 构建当前分支二进制，再按本用例执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_external_runner_live_guide.sh`。脚本从污染父环境启动 detached daemon，Codex/Traex/Claude 的 CLI、Web 和 mock Feishu IM guide 均通过，输出 `[external-runner-live-guide] PASS`；cleanup 后测试数据目录及所属进程均已回收。

## 清理步骤

```bash
# 停止测试服务
# Ctrl+C 或 kill 进程

# 清理测试数据目录
rm -rf ./.bifrost-test

# 清理测试 provider
curl -s -X DELETE http://127.0.0.1:8801/_bifrost/api/im-gateway/providers/test-p1
```
