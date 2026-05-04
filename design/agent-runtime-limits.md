# Agent Runtime Limits 与超时策略

## 功能模块说明

本次修复聚焦 Bifrost Agent runtime 在真实 IM Gateway / Admin API 对话中出现的提前退出问题，目标包括：

1. 将默认 `max_turn_iterations` 从偏小的开发期值提升到更接近真实代理执行需求的上限，避免长链路工具调用在 30 次左右被硬中断。
2. 将默认超时策略调整为更接近 Codex 的分层方案：保留较高的迭代上限以避免 30 次左右被硬中断，但模型请求、shell 工具、后台终端、MCP 启动与 MCP tool call 的默认超时保持在 10 分钟量级，避免默认值过长掩盖卡死与异常等待。
3. 去掉 `AgentClient` 内部与配置体系重复、且可能形成隐藏上限的 builder 级 300 秒超时，确保 runtime 只受显式配置控制。
4. 通过真实 Bifrost 进程 + Admin API + mock model server 验证：超过 30 轮工具调用的会话可以继续执行直至完成，不再报 `exceeded maximum iterations (30)`。

## 现状与问题

当前 runtime 存在以下限制：

- `crates/agent/src/config.rs`
  - `DEFAULT_MAX_TURN_ITERATIONS = 20`
  - `DEFAULT_REQUEST_TIMEOUT = 120`
  - `DEFAULT_SHELL_TIMEOUT = 30`
  - `DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS = 300_000`
- `crates/agent/src/mcp.rs`
  - `DEFAULT_STARTUP_TIMEOUT_SEC = 30`
  - `DEFAULT_TOOL_TIMEOUT_SEC = 60`
- `crates/agent/src/client.rs`
  - `reqwest::Client::builder().timeout(Duration::from_secs(300))`

虽然运行时配置允许用户覆盖这些值，但默认值和隐藏 builder timeout 仍然会导致以下问题：

- 初次启用 Agent 或临时数据目录下的新实例，默认就容易在较长工具链路中失败。
- 即使某些路径已经把 `request_timeout_secs` 提高，client builder 仍可能留下 300 秒硬上限，形成难以排查的“第二层超时”。
- MCP server 在初始化较慢或工具执行较长时，会因为 startup/tool timeout 过低被误判失败。

## 实现逻辑

### 1. 调整 AgentConfig 默认值

修改 `crates/agent/src/config.rs`：

- `DEFAULT_MAX_TURN_ITERATIONS = 1000`
- `DEFAULT_REQUEST_TIMEOUT = 600`
- `DEFAULT_SHELL_TIMEOUT = 600`
- `DEFAULT_BACKGROUND_TERMINAL_TIMEOUT_MS = 600_000`

并同步更新字段注释、默认构造值和相关测试断言，确保：

- `AgentConfig::default()` 直接体现新的 runtime 上限
- `get_*` accessor 的默认回退值与常量保持一致
- TOML 反序列化与 merge 行为不受影响

### 2. 统一 MCP 默认超时

修改 `crates/agent/src/mcp.rs`：

- `DEFAULT_STARTUP_TIMEOUT_SEC = 600`
- `DEFAULT_TOOL_TIMEOUT_SEC = 600`

保持现有行为不变：

- 显式配置仍然优先于默认值
- `start_one_server()` 内部继续使用 `startup_timeout_sec.unwrap_or(DEFAULT_STARTUP_TIMEOUT_SEC)` 和 `tool_timeout_sec.unwrap_or(DEFAULT_TOOL_TIMEOUT_SEC)`

### 3. 去掉 AgentClient 隐式 300 秒上限

修改 `crates/agent/src/client.rs`：

- `AgentClient::new()` 中移除 `reqwest::Client::builder().timeout(Duration::from_secs(300))`
- 保留每次请求上的：
  - `.timeout(Duration::from_secs(effective.request_timeout_secs))`

这样可以保证：

- 所有请求超时只由配置控制
- 不再存在 builder 默认值与 request-level timeout 冲突或相互覆盖的问题

### 4. 真实接口验证策略

新增黑盒 E2E 脚本，使用本地 mock model server 让模型连续发起 35 次以上的工具调用，再返回最终文本响应。

关键验证点：

1. Admin API 返回的默认配置已变为 1000 次迭代 / 600 秒级默认超时。
2. `POST /_bifrost/api/im-gateway/agent/chat` 在 35+ 轮工具调用后仍然成功结束。
3. 响应中 `tool_calls` 数量大于 30，证明不再被旧上限阻断。
4. 整体链路未出现 `exceeded maximum iterations (30)` 或旧默认 timeout 导致的异常。

## 依赖项

- `crates/agent/src/config.rs`
- `crates/agent/src/client.rs`
- `crates/agent/src/mcp.rs`
- `crates/bifrost-admin/src/handlers/im_gateway.rs`（复用现有 Agent Admin API 做黑盒验证）
- `e2e-tests/tests/` 下新增真实接口测试脚本
- `human_tests/` 下新增真实场景验证文档

## 测试方案

### 单元测试

1. `crates/agent/src/config.rs::test_default_config`
   - 验证默认 `shell_timeout_secs = 600`
   - 验证默认 `request_timeout_secs = 600`
   - 验证默认 `max_turn_iterations = 1000`
   - 验证默认 `background_terminal_max_timeout = 600000`
2. `crates/agent/src/mcp.rs` 现有默认配置回退路径相关测试
   - 验证 `startup_timeout_sec/tool_timeout_sec` 为 `None` 时仍能走新的默认值常量
3. 新增/更新 `crates/agent/src/client.rs` 相关测试（如果需要）
   - 验证 `AgentClient::new()` 不再写死 300 秒 builder timeout

### E2E 测试

新增 `e2e-tests/tests/test_agent_loop_runtime_limits.sh`，覆盖：

1. `GET /_bifrost/api/im-gateway/agent`
   - 断言默认 `max_turn_iterations = 1000`
   - 断言默认 `request_timeout_secs = 600`
   - 断言默认 `shell_timeout_secs = 600`
   - 断言默认 `background_terminal_max_timeout = 600000`
2. PATCH mock provider 后调用真实 `POST /_bifrost/api/im-gateway/agent/chat`
   - mock 连续触发 35 次 `list_directory` 工具调用
   - 最终成功返回文本响应
   - 响应中的 `tool_calls` 数量 > 30
   - 脚本整体 PASS

### 真实场景测试（human_tests）

新增 `human_tests/agent-loop-timeouts.md`，覆盖：

1. `TC-AL-01`：读取默认 Agent 配置，确认迭代上限和各类 timeout 已提升
2. `TC-AL-02`：执行真实黑盒脚本，验证超过 30 次工具调用的会话仍能完成
3. `TC-AL-03`：显式 PATCH 600 秒级请求 / shell / 后台终端配置并重新 GET，确认 API 层接受并持久化这些值

## 校验要求

提交前必须执行：

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. 相关单测
4. 新增 E2E 脚本
5. `cargo test --workspace --all-features`
6. `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

本次需要同步更新：

- `human_tests/agent-loop-timeouts.md`（新增）
- `human_tests/readme.md`（新增索引）
- 如测试脚本新增，需保持其命名与用途清晰可追踪
