# Agent Loop Runtime Limits 真实场景测试

## 功能模块说明

验证 IM Gateway Agent 的 runtime 默认限制已调整为更合理的方案：

- 保留较高的 `max_turn_iterations = 1000`，避免长链路工具调用在 30 次左右被硬中断
- 将模型请求、shell、后台终端、MCP 的默认超时统一收敛为 600 秒级，而不是过长的 24 小时
- 移除 `AgentClient` 中隐藏的 builder 级 300 秒超时后，真实请求超时仅由显式配置控制
- 通过真实 Bifrost 进程 + Admin API + 本地 mock model server 验证：35 次以上工具调用的对话仍能完成，不会报 `exceeded maximum iterations (30)`

## 前置条件

1. 当前目录位于仓库根目录
2. 本地已具备 Rust 构建环境
3. 测试端口避开正式环境 `9900`
4. 启动 Bifrost 时必须带 `--no-system-proxy`

## 测试用例列表

### TC-AL-01：读取默认 Agent 配置，确认默认 runtime 限制

**操作步骤**：
1. 运行 E2E Rust 用例：
   ```bash
   cargo test -p bifrost-e2e im_gateway_agent_config_get -- --nocapture
   ```
2. 或手工启动临时 Bifrost 后读取：
   ```bash
   TEST_DIR="$(mktemp -d)"
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
     --host 127.0.0.1 \
     -p 18895 \
     --unsafe-ssl \
     --no-system-proxy
   curl -s http://127.0.0.1:18895/_bifrost/api/im-gateway/agent | jq .
   ```

**预期结果**：
- 返回 JSON 包含：
  - `request_timeout_secs = 600`
  - `shell_timeout_secs = 600`
  - `max_turn_iterations = 1000`
  - `background_terminal_max_timeout = 600000`

**实际结果**：
- 通过。执行 `cargo run -p bifrost-e2e -- --test im_gateway_agent_config_get --test-timeout 180` 返回 1/1 PASS，确认默认 `request_timeout_secs = 600`、`shell_timeout_secs = 600`、`max_turn_iterations = 1000`、`background_terminal_max_timeout = 600000`。

---

### TC-AL-02：真实黑盒脚本验证 35+ 次工具调用不会在 30 次时提前中断

**操作步骤**：
1. 运行黑盒脚本：
   ```bash
   bash e2e-tests/tests/test_agent_loop_runtime_limits.sh
   ```

2. 脚本内部会完成以下动作：
   - 启动本地 mock model server
   - `cargo build --bin bifrost`
   - 用临时数据目录启动真实 Bifrost 进程
   - GET `/_bifrost/api/im-gateway/agent` 校验默认值
   - PATCH `/_bifrost/api/im-gateway/agent` 指向 mock provider
   - 调用真实 `POST /_bifrost/api/im-gateway/agent/chat`
   - 让 mock 连续触发 35 次 `list_directory`
   - 校验最终对话成功结束，且没有出现 `exceeded maximum iterations (30)`

**预期结果**：
- 脚本输出：
  ```text
  [agent-loop-runtime-limits] PASS
  ```
- 默认值断言通过：
  - `request_timeout_secs = 600`
  - `shell_timeout_secs = 600`
  - `max_turn_iterations = 1000`
  - `background_terminal_max_timeout = 600000`
- `/agent/chat` 返回：
  - `success = true`
  - `tool_calls` 数量 `>= 35`
  - 最终响应包含“已完成 35 次 list_directory 调用”
- Bifrost 日志中**不包含**：
  - `exceeded maximum iterations (30)`

**实际结果**：
- 通过。执行 `bash e2e-tests/tests/test_agent_loop_runtime_limits.sh` 输出 `[agent-loop-runtime-limits] PASS`；真实 `/agent/chat` 成功完成 35 次 `list_directory` 调用，脚本断言 `tool_calls >= 35` 且最终响应包含“已完成 35 次 list_directory 调用”，同时日志未出现 `exceeded maximum iterations (30)`。

---

### TC-AL-03：显式 PATCH 600 秒级请求 / shell / 后台终端配置并重新 GET

**操作步骤**：
1. 启动临时服务：
   ```bash
   TEST_DIR="$(mktemp -d)"
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
     --host 127.0.0.1 \
     -p 18897 \
     --unsafe-ssl \
     --no-system-proxy
   ```

2. PATCH 配置：
   ```bash
   curl -s -X PATCH http://127.0.0.1:18897/_bifrost/api/im-gateway/agent \
     -H 'Content-Type: application/json' \
     -d '{
       "request_timeout_secs": 600,
       "shell_timeout_secs": 600,
       "background_terminal_max_timeout": 600000,
       "max_turn_iterations": 1000
     }' | jq .
   ```

3. 再次 GET：
   ```bash
   curl -s http://127.0.0.1:18897/_bifrost/api/im-gateway/agent | jq .
   ```

**预期结果**：
- PATCH 返回完整 AgentConfig JSON
- 再次 GET 后相关字段保持：
  - `request_timeout_secs = 600`
  - `shell_timeout_secs = 600`
  - `background_terminal_max_timeout = 600000`
  - `max_turn_iterations = 1000`

**实际结果**：
- 待执行。

## 清理步骤

1. 删除测试过程中创建的临时目录
2. 确认没有残留的 mock model server 或 bifrost 进程
3. 如需再次执行，可直接重新运行上述命令
