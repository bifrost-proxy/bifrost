# Agent Loop 进程隔离

## 功能模块说明

Bifrost 主进程同时承载流量代理、Admin API、IM Gateway 与 Agent/Runner 调度。旧实现存在多条同进程执行路径：内置 Bifrost Agent loop、IM Agent loop、Web/Admin 非流式测试入口、内置 runner-call，以及 ChatGPT Web 这类外置 Runner 的浏览器/CDP 编排。任一路径在模型上下文构建、工具调用、文件处理、浏览器自动化或第三方 SDK 中出现 CPU 密集/同步阻塞时，都会挤占主进程 runtime，导致代理流量、Admin API、IM 消息处理和 `/stop` 一起失去响应。

本模块把默认 Agent/Runner 执行改为“每个会话一个独立进程”：主进程只负责接收用户输入、维护会话 busy/preview 状态、转发进度事件和返回最终结果；内置 Bifrost Agent 与外置 Runner（Codex、ChatGPT Web、自定义 CLI Runner）都在子进程中完成实际 loop/编排。

## 卡死原因分析

- 旧实现把 `run_turn_with_mcp_multimodal()` 放在主进程 Tokio task 中运行，`/stop` 依赖同进程 cooperative `AgentStopSignal`。
- 外置 Runner 虽然会启动 Codex 等 CLI 子进程，但 `ExternalCliRuntime::run()` 本身以及 ChatGPT Web adapter 的浏览器/CDP 自动化仍在主进程中执行。
- 一旦 loop 内部发生 CPU 密集、同步阻塞或浏览器自动化卡住，主进程 runtime 被饥饿，代理请求和 `/stop` HTTP/IM 事件也排不上执行。
- 即使 `/stop` 被处理，旧内存信号也只能在 loop 回到 await/checkpoint 后生效，不能强制中断同进程卡死代码。
- 根因是执行域和控制域共享主进程；必须把执行域迁移到可独立终止的子进程。

## 实现逻辑

1. 内置 Agent worker。
   - 新增隐藏子命令 `bifrost agent worker`。
   - stdin 接收 `run/guide/stop` JSON 命令，stdout 输出 `started/progress/finished/failed/stopped` NDJSON。
   - worker 内部执行 `run_turn_with_mcp_multimodal()`、MCP、send_msg/schedule 工具注册和 goal continuation。
   - Unix 下使用独立 process group，stop 超时后 kill worker 进程组。
2. 外置 Runner worker。
   - 新增隐藏子命令 `bifrost agent external-runner-worker`。
   - `ExternalCliRuntime::run()` 默认先启动 worker 子进程；只有 worker 内部通过 `BIFROST_EXTERNAL_CLI_WORKER=1` 执行原始 runtime。
   - Codex/custom CLI 的命令启动、ChatGPT Web adapter 的浏览器/CDP 编排均迁移到 worker 内。
   - worker stop 先发 stdio stop，再超时 kill worker 进程组，同时 worker 内 stop 会清理自身已启动的 external CLI 子进程。
3. 覆盖入口。
   - Web/Admin `/api/agent/chat/stream` 使用内置 Agent worker。
   - IM built-in Agent `process_agent_chat()` 使用内置 Agent worker，并把主进程 guide queue 转发为 worker `Guide` 命令；主进程等待 worker event 时必须同时监听 guide channel notification，避免 worker 正在等待模型响应或长工具期间 guide 被卡到下一次 worker event 才转发。
   - Web/Admin `/api/im-gateway/agent/chat` 非流式测试入口使用内置 Agent worker。
   - `bifrost-e2e` 自定义 E2E runner 在 CI 中作为 `current_exe()` 启动 worker 时，也必须支持隐藏 `agent worker` 与 `agent external-runner-worker` pass-through 入口，避免 in-process E2E 服务把 worker 子进程误启动为普通测试 runner。
   - Slash runner-call 目标为 built-in Agent 时使用内置 Agent worker。
   - Chat Gateway、IM Event Loop、Schedule、Daily Agent 等所有 `ExternalCliRuntime::run()` 调用默认进入外置 Runner worker。
4. 状态与 stop。
   - 主进程仍用 `AgentSessionManager` 做 busy gate 和 session preview。
   - worker progress 反向同步 active turn status、title、plan/progress card。
   - worker 与主进程各自持有 `ImGatewayService`，send_msg / schedule 这类工具写入的消息与定时任务落盘后，主进程读取 store 前必须重新加载磁盘数据，避免独立进程写入后 API/list 仍读到旧内存快照。
   - worker 恢复 JSONL history 时必须恢复 runtime state（plan、goal、token snapshot、compaction count、base instructions）和原始 source channel，避免续聊后 `/status`、plan、timeline run_state 与主进程恢复路径不一致。
   - `/stop` 聚合 internal cooperative signal、内置 Agent worker、外置 Runner worker、legacy external CLI run stop；`/_bifrost/api/im-gateway/agent/chat` 的 `/stop` 作为控制成功响应返回 200 + `stopped=true`，不把 worker stopped 当作 500。
   - `/clear`/`/reset` 在 `/_bifrost/api/im-gateway/agent/chat` 中同样走 session-free 控制路径：停止 active worker、清理内存 session/queue，并删除 built-in Agent adapter 持久化 session state 与 JSONL history，确保服务重启后不会恢复旧上下文。
   - SSE 断开或 stop 后清理 worker，避免孤儿进程。

## 依赖项

- `serde` / `serde_json`：worker stdio 协议序列化。
- `tokio::process`：主进程异步启动和管理 worker。
- `dashmap`：主进程 active worker/session 索引。
- `process_group(0)` 与跨平台 terminate 封装：stop/断开时终止 worker 及其子进程组。

## 测试方案

### 单元测试

- `agent_worker::build_run_request_uses_protocol_version_and_session`：验证 worker 请求携带协议版本、session、work_dir、source。
- `agent_worker::turn_result_roundtrip_preserves_stop_fields`：验证 worker result 与 `TurnResult` 转换不丢字段。
- `agent_worker::validate_request_rejects_bad_protocol`：验证协议版本不兼容时拒绝。
- `external_cli` targeted tests：验证 external CLI runtime、stop by run/session、Codex adapter、IM event loop external runner 仍通过；测试环境默认绕过 worker，真实 E2E 覆盖 worker 进程。

### E2E 测试

`e2e-tests/tests/test_agent_worker_process_isolation.sh`：

- 构建当前 bifrost。
- 用临时 `BIFROST_DATA_DIR` 启动服务，必须带 `--no-system-proxy`。
- 配置内置 Agent 慢速 mock 模型，发起 `/api/agent/chat/stream`。
- 断言独立 `bifrost agent worker` 子进程出现，主进程 Admin API 继续响应。
- 调用 `/stop`，断言 worker 退出且主进程继续响应。
- 断开 SSE，断言 worker 自动清理。
- 配置 slow mock external runner，发起 `/api/im-gateway/chat/stream`。
- 断言独立 `bifrost agent external-runner-worker` 子进程出现，主进程继续响应，`/stop` 可停止外置 Runner worker。

`cargo run -p bifrost-e2e -- --test im_gateway_agent_chat_`：

- 验证 `bifrost-e2e` 当前可执行文件支持隐藏 worker pass-through，内置 Agent worker 能在 in-process E2E Admin 服务中正常启动。
- 验证 `/agent/chat` 非流式测试入口中的 `/stop` 返回 200 + stopped 语义，原 active chat 收敛后 session 可继续使用。
- 验证 `/reset` 删除持久化 built-in Agent history，模拟服务重启后 fresh chat 不携带 reset 前消息。

CI shell E2E worker 隔离回归：

- `test_agent_send_msg_feishu_card.sh`：worker 进程通过 `send_msg` 工具写入 Feishu card 后，主进程 message log API 重新加载磁盘 store 并能立刻查到 outbound 记录。
- `test_agent_send_msg_default_channel.sh`：worker 使用默认消息通道发送消息后，主进程 message log / schedule 相关 store 不因进程内缓存缺失而返回空数据。
- `test_agent_chat_history_continue.sh`：worker 续聊恢复 JSONL runtime state，`plan_steps` 不丢失。
- `test_agent_direct_path_switch.sh`：worker 返回 `work_dir_switched` 后主进程 session state 立即更新，后续 `/status` 显示新工作目录。
- `test_agent_run_timeline_channel_unification.sh`：worker 写入 run_state 时保留请求来源 `api` / `web`，不把所有状态归因成 `worker` 或 `admin-api`。
- `test_im_guide_queue_human_api.sh`：内置 IM Agent active turn 阻塞在 worker/mock model 时，busy 普通 IM 消息默认进入 guide；guide notify 必须唤醒主进程并立即转发给 worker，不等待 worker 产生下一条 event。

### 真实场景测试

`human_tests/agent-loop-process-isolation.md`：

- TC-ALPI-01：内置 Web/Admin Agent 请求启动后出现独立 worker，主进程继续响应代理/Admin API。
- TC-ALPI-02：`/stop` 能停止内置 worker，无需强杀主进程。
- TC-ALPI-03：SSE 客户端断开后内置 worker 被清理。
- TC-ALPI-04：外置 Runner 请求启动后出现独立 external-runner worker，主进程继续响应。
- TC-ALPI-05：`/stop` 能停止外置 Runner worker。
- TC-ALPI-06：CI/E2E runner 进程作为 `current_exe()` 时可启动 worker，`/agent/chat` 的 `/stop` 和 `/reset` 控制语义保持 200 成功响应并清理持久化历史。
- TC-ALPI-07：worker 独立进程写入 send_msg/schedule/history/timeline/work_dir 后，主进程 API 读取到最新落盘状态，覆盖 CI shell E2E 失败路径。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认每个 Agent/Runner 会话独立进程；覆盖 built-in、IM、ChatGPT/Codex/custom runner；主进程只代理输入输出；stop 不依赖同进程 cooperative loop。
- Review 范围：`agent_worker.rs`、`external_cli/mod.rs`、Admin/IM Agent handlers、runner-call、stop 聚合、CLI 隐藏子命令、E2E/human_tests。
- 风险点：stdout 协议被日志污染、worker 进程组未隔离、session busy 未释放、外置 Runner stop 只停 orchestration 不停子 CLI、guide/queue 丢失或因主进程未监听 guide notify 而延迟到 worker event 后才转发、历史恢复不一致。
- 复测命令：`cargo fmt --all -- --check`、`cargo check -p bifrost-admin -p bifrost-cli`、`cargo test -p bifrost-admin agent_worker --lib`、`cargo test -p bifrost-admin external_cli --lib`、E2E、human_tests。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、未跟踪文件和 staged 状态。
- 检查文档、human_tests/readme、E2E 脚本、CLI 隐藏命令、stop 路径和用户目标一致。
- 复跑受影响测试与 workspace 兜底：targeted cargo、E2E、`cargo test --workspace --all-features`、clippy/build。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo check -p bifrost-admin -p bifrost-cli`
- `cargo test -p bifrost-admin agent_worker --lib`
- `cargo test -p bifrost-admin external_cli --lib`
- `bash e2e-tests/tests/test_agent_worker_process_isolation.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `cargo build --all-targets --all-features`
- 收尾执行 rust-project-validate 要求的验证矩阵。

## 文档更新要求

- 更新本设计文档。
- 更新并执行 `human_tests/agent-loop-process-isolation.md`。
- 更新 `human_tests/readme.md` 索引。
