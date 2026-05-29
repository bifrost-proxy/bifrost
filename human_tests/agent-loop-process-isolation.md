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
