# Agent Loop 进程隔离真实场景测试

## 功能模块说明

验证 Agent/Runner loop 默认不再运行在主进程内，而是按会话启动独立 worker 子进程。覆盖内置 Bifrost Agent、IM/Web Admin 入口、外置 Runner（Codex、ChatGPT Web、自定义 CLI Runner 同属 `external-runner-worker` 执行域），确保主进程只负责接收输入、转发进度/结果和 stop 控制，代理/Admin API 不因 Agent/Runner CPU 或同步阻塞而失去响应。

## 前置条件

1. 在仓库根目录执行。
2. 使用临时数据目录，禁止污染本机数据。
3. 启动服务必须使用 `--no-system-proxy`，本用例不涉及系统代理。
4. 使用当前源码构建出的 bifrost 二进制。

推荐准备命令：

```bash
cargo build --bin bifrost
TEST_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start -p 18881 --unsafe-ssl --skip-cert-check --no-system-proxy
```

## 测试用例列表

### TC-ALPI-01：内置 Agent 请求启动独立 worker 且主进程继续响应

操作步骤：

1. 配置临时 Agent 模型 endpoint 为慢速本地 mock，避免真实 LLM 依赖。
2. 发起流式 Agent 请求：
   ```bash
   curl -sS -N -X POST "http://127.0.0.1:18881/_bifrost/api/agent/chat/stream" \
     -H 'Content-Type: application/json' \
     -d '{"message":"hello","session_key":"human-agent-worker"}' >/tmp/bifrost-agent-worker-human.sse &
   ```
3. 查询进程列表：
   ```bash
   pgrep -af "bifrost agent worker"
   ```
4. 验证主进程 Admin API 仍响应：
   ```bash
   curl -sS "http://127.0.0.1:18881/_bifrost/api/proxy/address"
   ```

预期结果：

- 出现独立 `bifrost agent worker` 子进程。
- `/_bifrost/api/proxy/address` 返回 JSON，服务没有卡死。
- SSE 文件包含 `run_started` 或后续事件。

### TC-ALPI-02：/stop 能停止内置 worker 且不强杀主进程

操作步骤：

1. 在 TC-ALPI-01 的请求运行期间发送 stop：
   ```bash
   curl -sS -X POST "http://127.0.0.1:18881/_bifrost/api/agent/chat/stream" \
     -H 'Content-Type: application/json' \
     -d '{"message":"/stop","session_key":"human-agent-worker"}'
   ```
2. 等待 2 秒后检查 worker：
   ```bash
   sleep 2
   ! pgrep -af "bifrost agent worker" | grep -q human-agent-worker
   ```
3. 再次检查主进程 Admin API：
   ```bash
   curl -sS "http://127.0.0.1:18881/_bifrost/api/proxy/address"
   ```

预期结果：

- stop 响应包含 `stopped` 或“已请求停止当前 Agent loop”。
- 内置 Agent worker 子进程退出。
- 主进程仍然响应 Admin API，无需 `kill -9` 主进程。

### TC-ALPI-03：SSE 客户端断开后内置 worker 被清理

操作步骤：

1. 发起新的 Agent stream 请求：
   ```bash
   curl -sS -N -X POST "http://127.0.0.1:18881/_bifrost/api/agent/chat/stream" \
     -H 'Content-Type: application/json' \
     -d '{"message":"hello again","session_key":"human-agent-worker-disconnect"}' >/tmp/bifrost-agent-worker-disconnect.sse &
   CURL_PID=$!
   sleep 1
   ```
2. 主动断开 SSE 客户端：
   ```bash
   kill "$CURL_PID" 2>/dev/null || true
   sleep 2
   ```
3. 检查 worker 清理情况：
   ```bash
   ! pgrep -af "bifrost agent worker" | grep -q human-agent-worker-disconnect
   ```

预期结果：

- SSE 客户端断开后，内置 worker 不会长期残留。
- 主进程继续响应 Admin API。

### TC-ALPI-04：外置 Runner 请求启动独立 external-runner worker

操作步骤：

1. 配置 slow mock external runner，命令 `sleep 30` 后输出 assistant final。
2. 发起 Chat Gateway 流式请求：
   ```bash
   curl -sS -N -X POST "http://127.0.0.1:18881/_bifrost/api/im-gateway/chat/stream" \
     -H 'Content-Type: application/json' \
     -d '{"message":"hello external","sessionKey":"human-external-worker","runnerId":"slow-mock"}' >/tmp/bifrost-external-worker-human.sse &
   ```
3. 查询进程列表：
   ```bash
   pgrep -af "bifrost agent external-runner-worker"
   ```
4. 验证主进程 Admin API 仍响应：
   ```bash
   curl -sS "http://127.0.0.1:18881/_bifrost/api/proxy/address"
   ```

预期结果：

- 出现独立 `bifrost agent external-runner-worker` 子进程。
- 外置 Runner 的 orchestration 不在主进程中执行；Codex/custom CLI 子进程或 ChatGPT Web 浏览器自动化均由 worker 承载。
- 主进程继续响应 Admin API。

### TC-ALPI-05：/stop 能停止外置 Runner worker

操作步骤：

1. 在 TC-ALPI-04 的请求运行期间发送 stop：
   ```bash
   curl -sS -N -X POST "http://127.0.0.1:18881/_bifrost/api/im-gateway/chat/stream" \
     -H 'Content-Type: application/json' \
     -d '{"message":"/stop","sessionKey":"human-external-worker","runnerId":"slow-mock"}'
   ```
2. 等待 2 秒后检查 external runner worker：
   ```bash
   sleep 2
   ! pgrep -af "bifrost agent external-runner-worker" | grep -q human-external-worker
   ```
3. 再次检查主进程 Admin API：
   ```bash
   curl -sS "http://127.0.0.1:18881/_bifrost/api/proxy/address"
   ```

预期结果：

- stop 响应包含 `stopped` 或“已请求停止当前 Runner”。
- 外置 Runner worker 子进程退出。
- 主进程仍然响应 Admin API。

### TC-ALPI-06：E2E runner 当前可执行文件支持 worker 入口且 `/agent/chat` 控制语义不回归

操作步骤：

1. 执行：
   ```bash
   BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --port 18080 --jobs 1 --timeout 900 --test im_gateway_agent_chat_ --test-timeout 180
   ```
2. 观察输出中的 `im_gateway_agent_chat_codex_prompt_layers`、`im_gateway_agent_chat_stop_active_loop`、`im_gateway_agent_chat_queue_state_persists_for_refresh`、`im_gateway_agent_chat_multimodal_image_parts`、`im_gateway_agent_chat_restores_history_after_service_restart`。

预期结果：

- 命令退出码为 `0`，5 个用例全部通过。
- `bifrost-e2e` 作为 `current_exe()` 时能响应隐藏 `agent worker` 入口，worker 不会以参数错误退出。
- `/agent/chat` 的 `/stop` 返回 200 + `stopped=true`，不再因为 worker stopped 被包装成 500。
- `/agent/chat` 的 `/reset` 会清理 built-in Agent 持久化 history，模拟服务重启后的 fresh chat 不携带 reset 前消息。

### TC-ALPI-07：独立 worker 写入状态后主进程读取不 stale

操作步骤：

1. 使用当前构建出的 `target/debug/bifrost` 作为 `BIFROST_BIN`，跳过重复构建执行以下 CI shell 回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=19331 MOCK_PORT=19332 bash e2e-tests/tests/test_agent_send_msg_feishu_card.sh
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=19341 MOCK_PORT=19342 bash e2e-tests/tests/test_agent_send_msg_default_channel.sh
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=19351 MOCK_PORT=19352 bash e2e-tests/tests/test_agent_chat_history_continue.sh
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=19361 MOCK_PORT=19362 bash e2e-tests/tests/test_agent_direct_path_switch.sh
   SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost BIFROST_PORT=19371 MOCK_PORT=19372 bash e2e-tests/tests/test_agent_run_timeline_channel_unification.sh
   ```
2. 观察每个脚本的 PASS 输出。

预期结果：

- Feishu card / 默认通道 send_msg 用例能从主进程 message log API 读到 worker 进程写入的 outbound 记录。
- history continue 用例返回恢复后的 `plan_steps`，不为 `null`。
- direct path switch 后 `/status` 显示新工作目录。
- timeline run_state 同时包含 `api` 和 `web` 来源，不出现把 worker 子进程误作为用户渠道的状态。

### TC-ALPI-08：exec_command 停止时清理整个 pipe process group

操作步骤：

1. 执行聚焦单测：
   ```bash
   cargo test -p bifrost-agent exec_command_ctrl_c_terminates_pipe_process_group_children -- --nocapture
   ```
2. 观察测试中创建的 shell 后台 `sleep` 子进程是否随 session Ctrl-C 一起退出。

预期结果：

- 命令退出码为 `0`。
- 测试断言后台孙进程 PID 不再存在，证明 `exec_command` 不会只杀直接 shell child 后遗留孙进程。

### TC-ALPI-09：生产 worker 启动失败不回退到主进程执行

操作步骤：

1. 执行聚焦单测：
   ```bash
   cargo test -p bifrost-admin spawn_or_fallback_fails_closed_when_forced_worker_cannot_start --lib -- --nocapture
   ```
2. 执行正常测试 fallback 单测：
   ```bash
   cargo test -p bifrost-admin spawn_or_fallback_uses_in_process_worker_in_tests_without_force_env --lib -- --nocapture
   ```

预期结果：

- 强制 worker 且可执行文件缺失时返回启动错误，不进入进程内 Agent loop。
- 未强制 worker 的测试路径仍可使用 in-process worker，避免单测需要依赖真实外部 worker。

## 清理步骤

```bash
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost -p 18881 stop || true
pkill -f "bifrost agent worker" || true
pkill -f "bifrost agent external-runner-worker" || true
rm -rf "$TEST_DIR"
rm -f /tmp/bifrost-agent-worker-human.sse /tmp/bifrost-agent-worker-disconnect.sse /tmp/bifrost-external-worker-human.sse
```

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-05-29 | TC-ALPI-01/02/03/04/05 | 执行 `bash e2e-tests/tests/test_agent_worker_process_isolation.sh`，脚本使用临时 `BIFROST_DATA_DIR`、慢速本地 mock 模型、slow mock external runner、`--unsafe-ssl --skip-cert-check --no-system-proxy` 启动当前构建 bifrost，验证内置 worker、外置 runner worker、`/stop`、SSE 断开清理和 Admin API 存活 | 通过 |
| 2026-05-29 | TC-ALPI-06 | 执行 `BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --port 18080 --jobs 1 --timeout 900 --test im_gateway_agent_chat_ --test-timeout 180`，验证 `bifrost-e2e` 当前可执行文件可作为 worker pass-through，且 `/agent/chat` `/stop` 与 `/reset` 控制语义不回归 | 通过：5/5 passed |
| 2026-05-29 | TC-ALPI-01/02/03/04/05 | 重新执行 `bash e2e-tests/tests/test_agent_worker_process_isolation.sh`，覆盖本轮 worker state 修复后内置/外置 worker、stop、SSE 断开清理和 Admin API 存活 | 通过 |
| 2026-05-29 | TC-ALPI-07 | 依次执行 `test_agent_send_msg_feishu_card.sh`、`test_agent_send_msg_default_channel.sh`、`test_agent_chat_history_continue.sh`、`test_agent_direct_path_switch.sh`、`test_agent_run_timeline_channel_unification.sh`，均使用 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost` 和独立临时端口/数据目录 | 通过：5/5 PASS |
| 2026-06-08 | TC-ALPI-08 | 执行 `cargo test -p bifrost-agent exec_command_ctrl_c_terminates_pipe_process_group_children -- --nocapture`，验证 pipe exec session 停止时会清理同 process group 的后台 `sleep` 孙进程 | 通过：1/1 passed |
| 2026-06-08 | TC-ALPI-09 | 执行 `cargo test -p bifrost-admin spawn_or_fallback --lib -- --nocapture`，验证测试 fallback 仍可控，且强制 worker 启动失败时返回错误、不进入主进程 loop | 通过：2/2 passed |
| 2026-06-08 | TC-ALPI-01/02/03/04/05 | 执行 `bash e2e-tests/tests/test_agent_worker_process_isolation.sh`，使用当前源码构建 bifrost 并以临时数据目录、`--no-system-proxy` 验证内置/外置 worker、stop、SSE 断开清理和 Admin API 存活 | 通过：`[agent-worker-process-isolation-e2e] PASS` |
