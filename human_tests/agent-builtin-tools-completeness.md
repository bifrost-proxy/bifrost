# Agent Builtin Tools Completeness 真实场景测试用例

## 功能模块说明

覆盖 Bifrost agent 对齐 Codex 编程内置工具的第一批能力：

1. `exec_command` 短命令一次性完成
2. `exec_command` 长任务返回 session，并由 `write_stdin` 继续交互
3. `view_image` 读取本地图片并返回 data URL
4. `request_user_input` 参数校验与当前不可交互边界
5. Codex 式 `tool_search` 只在 deferred tools 存在时暴露，并返回可加载工具定义
6. `ChatMessage` 手写序列化类型在 workspace all-features 下可编译
7. 启动真实 Bifrost 服务，通过 `/api/im-gateway/agent/chat` 触发真实模型对话和默认直暴工具调用
8. 本地 CI 静态门禁覆盖 IM gateway 编译回归
9. MCP 工具数量达到 Codex 阈值 `>= 100` 时进入 deferred loading
10. 真实 Bifrost + 大量 MCP tools 验证 `tool_search` 搜索、加载、调用策略生效

## 前置条件

```bash
cd <REPO_ROOT>
export CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target
mkdir -p ./.bifrost-test
```

## 测试用例列表

### TC-ABT-01: `exec_command` 短命令一次性完成

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent exec_command_returns_completed_output -- --nocapture
  ```
- **预期结果**: 测试通过；`exec_command` 返回 JSON，包含 `exit_code: 0`、`output: "hello"`，且不返回持续会话。
- **本次执行结果**: 2026-05-06 执行通过；`exec_command_returns_completed_output` 测试返回 ok。

### TC-ABT-02: `exec_command` 交互任务通过 `write_stdin` 继续

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end -- --nocapture
  ```
- **预期结果**: 测试通过；短命令返回完成 JSON；交互命令返回 `session_id`，`write_stdin` 使用 Codex-compatible `chars` 字段写入后可读回 `hello exec`。
- **本次执行结果**: 2026-05-06 执行通过；`exec_command_tool_works_end_to_end` 测试返回 ok。

### TC-ABT-03: `view_image` 返回本地图片 data URL

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent view_image -- --nocapture
  ```
- **预期结果**: 测试通过；PNG 文件返回 `data:image/png;base64,` 前缀；缺失图片返回明确错误。
- **本次执行结果**: 2026-05-06 执行通过；`view_image_rejects_missing_file`、`view_image_returns_data_url`、`view_image_tool_works_end_to_end` 测试均返回 ok。

### TC-ABT-04: `request_user_input` 校验参数并明确当前不可交互

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent request_user_input -- --nocapture
  ```
- **预期结果**: 测试通过；空问题列表被拒绝；合法问题返回当前 runtime 不支持交互等待的明确错误。
- **本次执行结果**: 2026-05-06 执行通过；`request_user_input_validates_questions`、`request_user_input_returns_unavailable_after_valid_request` 测试均返回 ok。

### TC-ABT-05: `tool_search` 遵循 Codex deferred 暴露语义

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent tool_search -- --nocapture
  ```
- **预期结果**: 测试通过；默认本地 `ToolRegistry` 不包含 `tool_search`；存在 deferred MCP entries 时，查询 `exec` 能返回可加载的 MCP `ToolDefinition`；`limit=0` 返回明确错误。
- **本次执行结果**: 2026-05-06 执行通过；`tool_search_rejects_zero_limit`、`tool_search_returns_loadable_deferred_tool_definitions`、`tool_search_is_hidden_without_deferred_tools`、`tool_search_returns_deferred_mcp_tools` 均返回 ok。

### TC-ABT-06: workspace all-features 可编译 `ChatMessage` 手写序列化类型

- **操作步骤**:
  ```bash
  cargo test --workspace --all-features
  ```
- **预期结果**: workspace 全 feature 测试通过编译；`crates/agent/src/types.rs` 中 `ChatMessage` 不再因无效字段级 `#[serde(...)]` 属性导致编译失败。
- **本次执行结果**: 2026-05-06 执行通过；workspace 全 feature 测试完成，`ChatMessage` 编译回归已修复。

### TC-ABT-07: 真实 Bifrost 服务 chat 端到端触发工具

- **操作步骤**:
  ```bash
  BIFROST_DATA_DIR=./.bifrost-test/agent-builtin-tools-live cargo run --bin bifrost -- start -p 18880 --unsafe-ssl --no-system-proxy
  curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/agent/tools
  curl -sS -X POST http://127.0.0.1:18880/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"agent-tools-live-e2e","work_dir":"<REPO_ROOT>","message":"请只使用当前可见工具完成验证：1) 用 exec_command 执行 printf BIFROST_TOOL_OK；2) 用 exec_command 创建一个小 PNG 文件到 ./.bifrost-test/agent-live.png；3) 用 view_image 读取这个 PNG；最后回答包含 BIFROST_TOOL_OK 和 image ok。不要使用 mock 数据。"}'
  curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/agent/sessions/agent-tools-live-e2e
  ```
- **预期结果**: 服务由当前源码构建启动；默认无 deferred MCP 时工具列表包含 `exec_command`、`write_stdin`、`view_image`，不包含 `tool_search`；chat 返回 `success: true`，`tool_calls` 中包含 `exec_command`、`view_image`，且最终响应或会话详情包含 `BIFROST_TOOL_OK`。
- **本次执行结果**: 2026-05-06 执行通过；当前源码构建的 Bifrost 在 18880 端口以 `BIFROST_DATA_DIR=./.bifrost-test/agent-builtin-tools-live` 和 `--no-system-proxy` 启动成功；工具列表包含 `exec_command`、`write_stdin`、`view_image`，不包含 `tool_search`；真实 chat 返回 `success: true`，`tool_calls` 记录包含 `set_title`、两次 `exec_command`、`view_image`，最终响应包含 `BIFROST_TOOL_OK` 和 `image ok`，会话详情记录 10 条消息和 3 条工具结果。

### TC-ABT-08: 本地 CI 静态门禁通过 IM gateway 编译回归

- **操作步骤**:
  ```bash
  bash scripts/ci/local-ci.sh --skip-e2e
  ```
- **预期结果**: `cargo fmt (workspace)`、`cargo fmt (desktop)`、`cargo clippy`、`cargo test (workspace)` 全部通过；`crates/bifrost-admin/src/handlers/im_gateway.rs` 不再出现 `String`/`&str` 类型不匹配，`crates/bifrost-admin/src/im_gateway/feishu.rs` 不再出现 `truncate_for_log` 未定义。
- **本次执行结果**: 2026-05-06 执行通过；`bash scripts/ci/local-ci.sh --skip-e2e` 最终报告为 4 passed、0 failed、5 skipped，覆盖 workspace fmt、desktop fmt、clippy、workspace all-features test。

### TC-ABT-09: MCP 工具数达到 Codex 阈值时进入 deferred loading

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent mcp_tool_exposure -- --nocapture
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent --test p1_tools_e2e tool_search -- --nocapture
  ```
- **预期结果**: 测试通过；MCP 工具数低于 100 时全部直接暴露；等于 100 时所有 MCP 工具进入 deferred；存在 deferred MCP 时 `tool_search` 返回可加载 `ToolDefinition`，下一轮模型请求可见。
- **本次执行结果**: 2026-05-06 执行通过；`mcp_tool_exposure_keeps_below_threshold_direct`、`mcp_tool_exposure_defers_at_codex_threshold`、`tool_search_is_hidden_without_deferred_tools`、`tool_search_returns_deferred_mcp_tools` 均返回 ok。

### TC-ABT-10: 真实 Bifrost 注册 100 个 MCP tools 后通过 `tool_search` 搜索并调用目标工具

- **操作步骤**:
  ```bash
  BIFROST_DATA_DIR=./.bifrost-test/agent-many-mcp-live cargo run --bin bifrost -- start -p 18881 --unsafe-ssl --no-system-proxy
  curl -sS -X PATCH http://127.0.0.1:18881/_bifrost/api/im-gateway/agent \
    -H 'Content-Type: application/json' \
    -d '{"mcp_servers":{"manytools":{"command":"node","args":["/Users/eden/work/github/bifrost/e2e-tests/mock_servers/many_mcp_tools_server.js"],"startup_timeout_sec":10,"tool_timeout_sec":10}}}'
  curl -sS -X POST http://127.0.0.1:18881/_bifrost/api/im-gateway/agent/chat \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"agent-many-mcp-tool-search-live","work_dir":"/Users/eden/work/github/bifrost","message":"请严格验证 MCP deferred tool discovery：先用 tool_search 搜索 needle_087，然后调用搜索结果中对应的 MCP 工具，参数 marker 填 LIVE_MARKER_087。最终回答必须包含 MCP_TOOL_087_OK 和 LIVE_MARKER_087。不要使用 mock 数据，不要用 exec_command 代替 MCP 工具。"}'
  ```
- **预期结果**: 当前源码构建的 Bifrost 在非 9900 端口启动；MCP server 初始化 100 个工具；chat 返回 `success: true`；`tool_calls` 包含 `tool_search` 和 `mcp_manytools__target_tool_087`；最终响应或工具结果包含 `MCP_TOOL_087_OK` 与 `LIVE_MARKER_087`。
- **本次执行结果**: 2026-05-06 执行通过；当前源码构建的 Bifrost 在 18881 端口以 `BIFROST_DATA_DIR=./.bifrost-test/agent-many-mcp-live` 和 `--no-system-proxy` 启动成功；通过真实 `PATCH /_bifrost/api/im-gateway/agent` 注册 stdio MCP server `manytools`，fixture 路径为 `e2e-tests/mock_servers/many_mcp_tools_server.js`，该 server 的 `tools/list` 返回 100 个工具；真实 chat 返回 `success: true`，`tool_calls` 依次包含 `set_title`、`tool_search`、`mcp_manytools__target_tool_087`；`tool_search` 结果只返回 `mcp_manytools__target_tool_087` 的可加载 `ToolDefinition`；MCP 工具调用结果为 `MCP_TOOL_087_OK marker=LIVE_MARKER_087`，最终响应包含 `MCP_TOOL_087_OK` 和 `LIVE_MARKER_087`。

## 清理步骤

```bash
rm -rf ./.bifrost-test/agent-builtin-tools-target
rm -rf ./.bifrost-test/agent-builtin-tools-live
```
