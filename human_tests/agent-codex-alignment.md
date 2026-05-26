# Agent Codex Alignment

## 功能模块说明

验证 Bifrost Agent 在不实现权限、沙箱、审批能力的前提下，继续对齐核心工具命名、MCP resource 暴露、旧 terminal 工具拒绝、事件驱动 turn runtime、并发工具批调度、`exec_command`/`write_stdin` 终端协议和后台长任务结束感知；同时确认默认 prompt 不暴露内部实现说明。

## 前置条件

- 在仓库根目录 `~/work/github/bifrost` 执行。
- 每条命令执行前先 `source ~/.zshrc`。
- TC-ACA-07 会启动临时 Bifrost 服务，必须使用临时 `BIFROST_DATA_DIR` 且带 `--no-system-proxy`，不修改系统代理。

## 测试用例列表

### TC-ACA-01 默认 prompt 不暴露 Codex 兼容实现说明

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && ! rg -n "mirrors Codex|permission escalation|internal implementation metadata" crates/agent/src/prompts/base_instructions/default.md
   ```

预期结果：

- 命令退出码为 0。
- 默认 prompt 不包含面向模型解释“mirrors Codex”、权限边界或内部实现元数据的说明。
- 权限、沙箱、审批兼容字段由工具 schema 和执行逻辑处理，不写进默认 prompt。

### TC-ACA-02 MCP resource 工具使用 Codex canonical 名称

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent mcp::resources::tests::test_all_resource_tool_definitions_are_function_tools
   ```

预期结果：

- 测试通过。
- 测试断言 `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` 三个 canonical 名称。

### TC-ACA-03 历史 terminal 工具不可见且不可执行

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent tools::tests::unknown_shell_aliases_are_rejected
   ```

预期结果：

- 测试通过。
- 默认模型可见工具列表不包含 `shell`、`shell_pty`、`shell_command` 和 `local_shell`。
- `shell`、`shell_pty`、`shell_command`、`local_shell` 旧名称执行失败并返回 `unknown tool`。

### TC-ACA-04 P1 工具链未被破坏

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent --test p1_tools_e2e
   ```

预期结果：

- `exec_command` + `write_stdin` E2E 通过。
- 无 deferred tools 时 `tool_search` 不可见。
- 有 deferred MCP tools 时 `tool_search` 能返回可加载工具定义。

### TC-ACA-05 并发 tool batch 保持 history 顺序

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent session::tests::codex_parallel_tool_batch_preserves_history_order
   ```

预期结果：

- 测试通过。
- 两个 barrier local tools 能在同一批内互相释放，证明不是顺序阻塞执行。
- `tool_calls_log` 和 Chat Completions history 中的 tool result 顺序仍为 `parallel_a`、`parallel_b`。
- `AgentSession.last_turn_events` 中包含 `ToolBatchStarted` 且 detail 为 `mode=parallel,count=2`，最终包含 `TurnCompleted`。

### TC-ACA-06 状态型工具不进入并发批

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent turn_runtime
   ```

预期结果：

- 测试通过。
- `tool_search`、`write_stdin`、Goal、`update_plan`、`set_title`、`switch_workdir` 等状态型/会话型工具判定为 ordered。
- 普通本地工具仍判定为 parallel。

### TC-ACA-07 真实 Bifrost 服务 `/agent/chat` 覆盖 Codex alignment 改动

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && ADMIN_PORT=18917 MOCK_HTTP_PORT=18918 bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh
   ```

预期结果：

- 脚本先执行 `cargo build --bin bifrost`，再用临时 `BIFROST_DATA_DIR` 启动真实 Bifrost 服务，启动参数包含 `--no-system-proxy`。
- 脚本启动 OpenAI-compatible mock model server 和 stdio MCP fixture server。
- 通过真实 `POST /_bifrost/api/im-gateway/agent/chat` 调用进入 agent loop。
- mock 捕获到首个模型请求：
  - prompt 不包含 `mirrors Codex`、权限边界说明或内部实现元数据。
  - tools 包含 `exec_command`、`write_stdin`、`tool_search`、`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`。
  - tools 不包含 `shell` / `shell_pty` / `shell_command` / `local_shell`。
  - tools 不直接包含超过阈值后被 deferred 的 `mcp_mcpfixture__sample_tool_*`。
- 模型返回同一批 tool calls：两个 1.5 秒 `exec_command`、一个 `update_plan`、一个 `set_title`、一个 `tool_search`、`list_mcp_resources`、`read_mcp_resource`。
- 第二轮模型调用继续启动一个长任务 `exec_command`，初次 `yield_time_ms` 后返回数字 `session_id` 且 `exit_code=null`。
- mock 在不主动轮询期间等待长任务结束，runtime watcher 应收敛最终 `WATCH_DONE` 与 `exit_code=0`，证明后台 watcher 已感知进程真实结束且模型不需要主动空轮询。
- `/agent/chat` 最终响应 `success=true`，包含 `CHAT_CODEX_ALIGNMENT_OK`。
- `tool_calls` 顺序为 `exec_command`、`exec_command`、`update_plan`、`set_title`、`tool_search`、`list_mcp_resources`、`read_mcp_resource`、`exec_command`，全部成功；首个模型请求仍暴露 `write_stdin` 工具，但最终轨迹不要求模型主动 poll。
- 第二个模型请求中的 tool result 顺序保持与 tool_calls 一致，且包含 `parallel-a`、`parallel-b`、计划输出、`SET_TITLE:Codex Alignment Real Chat`、`sample_tool_042`、`bifrost://codex-alignment`、`MCP_RESOURCE_OK`。
- 总耗时低于 7000ms，证明两个 `exec_command` tool 在同一批内并发执行，同时覆盖后台长任务由 runtime watcher 获取最终 exit code。

### TC-ACA-09 `exec_command` / `write_stdin` Codex unified exec 语义对齐

操作步骤：

1. 执行：

   ```bash
   source ~/.zshrc && cargo test -p bifrost-agent tools::exec_command -- --nocapture
   source ~/.zshrc && cargo test -p bifrost-agent turn_runtime::tests::stateful_tools_are_ordered -- --nocapture
   ```

预期结果：

- `write_stdin` 被判定为 ordered，不进入并发 local tool batch。
- `exec_command` 默认 `yield_time_ms` 为 10000ms，`write_stdin` 默认非空输入为 250ms；空输入 poll 至少等待 5000ms，并受 `background_terminal_max_timeout` 上限控制。
- 长任务在初始 yield 后返回持续 session；即使模型没有立即再次 poll，后台 watcher 也能观察到真实进程结束，后续 `write_stdin` 返回最终 `exit_code` 和尾部输出。

## 清理步骤

- TC-ACA-07 使用脚本内创建的临时数据目录、mock model server 和 stdio MCP fixture；脚本退出时通过 trap 清理临时服务和目录。

## 本轮执行记录

| 用例 | 状态 | 日期 | 实际结果 |
| --- | --- | --- | --- |
| TC-ACA-01 | 通过 | 2026-05-11 | `! rg -n "mirrors Codex\|permission escalation\|internal implementation metadata" crates/agent/src/prompts/base_instructions/default.md` 通过，确认默认 prompt 已移除内部实现说明。 |
| TC-ACA-02 | 通过 | 2026-05-11 | `cargo test -p bifrost-agent mcp::resources::tests::test_all_resource_tool_definitions_are_function_tools` 通过：1 passed；补充执行 `cargo test -p bifrost-agent mcp::tests::mcp_resource_tools_stay_direct_when_server_tools_are_deferred` 通过：1 passed，确认 resource tools 不随 server tools deferred。 |
| TC-ACA-03 | 通过 | 2026-05-12 | `cargo test -p bifrost-agent --test p1_tools_e2e legacy_shell_tools_are_not_registered_by_default -- --nocapture` 通过；确认 `shell` / `shell_pty` 不注册且执行失败。`cargo test -p bifrost-agent tools::tests::unknown_shell_aliases_are_rejected` 覆盖 `shell_command` / `local_shell`。 |
| TC-ACA-04 | 通过 | 2026-05-11 | `cargo test -p bifrost-agent --test p1_tools_e2e` 通过：7 passed、1 ignored（本机 Codex CLI interactive 手工回归用例按既有标记忽略）。 |
| TC-ACA-05 | 通过 | 2026-05-11 | `cargo test -p bifrost-agent session::tests::codex_parallel_tool_batch_preserves_history_order` 通过：1 passed；barrier 工具证明 local tool batch 并发执行，history 顺序保持 `parallel_a`、`parallel_b`。 |
| TC-ACA-06 | 通过 | 2026-05-12 | `cargo test -p bifrost-agent turn_runtime::tests::stateful_tools_are_ordered -- --nocapture` 通过：1 passed；状态型工具和 `write_stdin` 保持 ordered，普通本地工具可 parallel。 |
| TC-ACA-07 | 通过 | 2026-05-25 | `ADMIN_PORT=18917 MOCK_HTTP_PORT=18918 bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh` 真实构建路径功能通过；自适应长任务改造后更新断言，首个模型请求仍暴露 `write_stdin`，但最终 `tool_calls` 不再要求模型主动空轮询，后台长任务由 runtime watcher 收敛最终 `exit_code=0`。脚本以临时数据目录和 `--no-system-proxy` 启动真实 Bifrost 服务，通过真实 `/agent/chat` 覆盖默认 prompt 不泄露兼容实现说明、MCP resource canonical tools、deferred MCP tools 通过 `tool_search` 发现但不直接暴露、`update_plan`、`set_title`、MCP resource read、并发 `exec_command` tool batch、history 顺序回填和 runtime watcher 长任务完成。 |
| TC-ACA-08 | 通过 | 2026-05-11 | CI 回归验证：`SKIP_BUILD=true ADMIN_PORT=18933 MOCK_HTTP_PORT=18934 bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh` 通过，确认 Agent chat E2E 在 CI 预构建 `target/release/bifrost` 场景下不再错误回退到缺失的 `target/debug/bifrost`。 |
| TC-ACA-09 | 通过 | 2026-05-12 | `cargo test -p bifrost-agent tools::exec_command -- --nocapture` 通过：10 passed；覆盖默认/clamp、后台 watcher、pipe/PTY/stdin/Ctrl-C 和 legacy 字段拒绝。`cargo test -p bifrost-agent turn_runtime::tests::stateful_tools_are_ordered -- --nocapture` 通过：1 passed；确认 `write_stdin` ordered。 |
