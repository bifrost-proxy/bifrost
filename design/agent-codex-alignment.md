# Agent Codex Alignment

## 功能模块说明

本模块用于把 Bifrost Agent 的 agent loop、prompt 分层和核心工具面持续对齐 `/Users/eden/work/github/codex` 的 Codex 设计风格。对齐目标不是照搬 Responses API，而是在 Bifrost 当前 Chat Completions 架构下保证模型看到的行为契约、工具命名、上下文边界和错误提示尽量一致。

本轮明确排除本地权限升级、OS sandbox 和审批工作流。Bifrost 暂时不实现这些能力；相关字段不进入工具 schema，也不把这类实现说明写进默认 prompt。

## 用户目标验证清单

### 必须实现

- Prompt 风格对齐：默认指令只描述当前 Agent 行为，不写内部兼容说明。
- 核心工具命名对齐：MCP resource 工具使用 Codex canonical 名称 `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`。
- 核心工具暴露对齐：MCP resource 工具在存在 MCP server 时保持直接可见，不进入 deferred `tool_search`。
- Shell 工具面收敛：只保留当前注册工具名，删除 `shell_command` / `local_shell` 历史 alias。
- MCP resource 执行对齐：resource list/read 调用走 MCP `resources/list`、`resources/templates/list`、`resources/read`，输出包含 server 字段，便于模型后续 read。
- Loop 架构对齐：turn 内部记录 Codex-style event stream，不再只能通过同步 Chat Completions loop 推断过程。
- 工具调度对齐：本地可并发工具使用 `FuturesOrdered` 批量执行，结果按模型 tool_calls 顺序落回 history；状态型工具保持顺序执行。

### 必须不破坏

- 不改变现有 `exec_command`、`shell_pty`、`write_stdin`、`apply_patch` 的可见顺序和既有行为。
- 不把权限、沙箱、审批伪实现成空成功；相关能力仍然不支持。
- 不让 MCP server tool 数量阈值逻辑改变：server tools 仍按 Codex `>= 100` 进入 deferred loading。
- 历史 shell alias 不再可见也不再可执行，避免扩大隐藏工具面。
- 不让并发工具结果按完成时间写入 history；必须保持模型调用顺序，避免 Chat Completions tool message 序列损坏。
- 不把 `tool_search`、Goal、计划、标题、工作目录切换等状态型工具并发化。

### 必须真实验证

- Rust 单元测试验证 MCP resource schema 名称、additionalProperties 边界和 ToolDefinition 转换。
- Rust 单元测试验证 `shell_command` / `local_shell` 会被拒绝，而不是映射到 `shell`。
- Rust 单元测试验证并发本地工具不会互相阻塞，且 history 按 tool_call 原顺序回填。
- Rust 单元测试验证状态型工具的执行模式保持 ordered。
- 目标 E2E 验证 agent P1 工具链仍可用。
- human_tests 文档覆盖默认 prompt 不泄露内部兼容说明、MCP resource canonical 名称、历史 shell alias 已移除、并发工具批调度与事件流。

### 必须交付

- 更新 design 文档。
- 更新对应 human_tests 与 readme 索引。
- 执行至少两轮 Review/Fix/Test。
- 执行 e2e-test 技能范围验证，再执行 rust-project-validate 技能范围验证。

## 实现逻辑

### Prompt 分层

Bifrost 仍保持当前 Chat Completions prompt 分层：

- system：base instructions。
- developer：配置级 developer instructions 与 skill metadata。
- user：用户自定义 instructions 与环境上下文。

本轮不在 base instructions 里加入兼容说明。默认 prompt 只保留面向 agent 的工作契约；权限、沙箱、审批这些暂不支持的字段不进入工具 schema。

### MCP resource 工具

Codex 把 MCP resources 作为核心工具注册，名称固定为：

- `list_mcp_resources`
- `list_mcp_resource_templates`
- `read_mcp_resource`

Bifrost 原先 resource helper 使用 `mcp__list_resources` 等非 canonical 名称，而且没有进入 agent turn 的工具暴露路径。本轮改为：

- resource tool definition 使用 canonical 名称和 Codex 风格描述。
- 当 MCP manager 有连接时，resource tools 直接加入 `McpToolExposure.direct_tools`。
- server tools 仍独立按 `DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD = 100` 决定 direct/deferred。
- `McpManager::call_tool` 对 resource tools 做虚拟路由，不依赖 `tool_routing`。

### Shell 工具面

Bifrost 只保留当前注册工具名：`shell`、`shell_pty`、`write_stdin`、`exec_command`。`shell_command` / `local_shell` 历史 alias 不再进入 `ToolRegistry::execute`，模型或外部调用如果继续使用这些旧名称会收到 `unknown tool`，从而尽早暴露旧协议。

## 依赖项

- `crates/agent/src/prompts/base_instructions/default.md`
- `crates/agent/src/tools/mod.rs`
- `crates/agent/src/turn_runtime.rs`
- `crates/agent/src/session.rs`
- `crates/agent/src/mcp/resources.rs`
- `crates/agent/src/mcp/mod.rs`
- `crates/agent/src/mcp/mod_tests.rs`
- `crates/agent/tests/p1_tools_e2e.rs`
- `human_tests/agent-codex-alignment.md`
- `human_tests/mcp-elicitation-resources.md`
- `human_tests/readme.md`

## 测试方案

### 单元测试

- `mcp::resources::tests::test_list_resources_tool_def_structure`：验证 canonical `list_mcp_resources` 名称、描述、schema 和 `additionalProperties=false`。
- `mcp::resources::tests::test_list_resource_templates_tool_def_structure`：验证 canonical `list_mcp_resource_templates`。
- `mcp::resources::tests::test_read_resource_tool_def_structure`：验证 canonical `read_mcp_resource` 和 required server/uri。
- `mcp::resources::tests::test_all_resource_tool_definitions_are_function_tools`：验证 resource helpers 能转成 Chat Completions `ToolDefinition`。
- `mcp::tests::mcp_resource_tools_stay_direct_when_server_tools_are_deferred`：验证 server tools 达到 deferred threshold 时 resource tools 仍直接可见。
- `tools::tests::unknown_shell_aliases_are_rejected`：验证 `shell_command` / `local_shell` 不可见且旧名称执行失败。
- `turn_runtime::tests::stateful_tools_are_ordered`：验证 `tool_search`、Goal、计划、标题、工作目录切换等工具保持 ordered。
- `turn_runtime::tests::ordinary_local_tools_can_run_in_parallel`：验证普通本地工具判定为 parallel。
- `session::tests::codex_parallel_tool_batch_preserves_history_order`：用两个 barrier 测试工具证明 local tool batch 并发执行，且 tool result 按 `response.tool_calls` 原顺序落回 history。

### E2E 测试

- `cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end`
- `cargo test -p bifrost-agent --test p1_tools_e2e tool_search_is_hidden_without_deferred_tools`
- `cargo test -p bifrost-agent --test p1_tools_e2e tool_search_returns_deferred_mcp_tools`
- `bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh`

这些用例覆盖本轮核心工具对齐不破坏既有 P1 工具链。

`test_agent_codex_alignment_chat_api.sh` 是本轮关键真实链路验证：脚本先编译 `bifrost`，再用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy` 启动真实服务，配置 mock Chat Completions provider 与 stdio MCP fixture，通过真实 `/agent/chat` 覆盖默认 prompt 不泄露兼容实现说明、MCP resource canonical tools、deferred MCP tools 通过 `tool_search` 发现但不直接暴露、`update_plan`、`set_title`、MCP resource read、并发 shell tool batch 和 history 顺序回填。

### 真实场景测试

创建 `human_tests/agent-codex-alignment.md`，至少覆盖：

- 默认 prompt 不包含权限、沙箱、审批兼容说明。
- 默认工具列表不包含 `shell_command` / `local_shell`，且旧 alias 执行会被拒绝。
- MCP resource schema 使用 canonical Codex 名称。
- Agent loop 记录 Codex-style turn events。
- 本地可并发工具通过 `FuturesOrdered` 并发执行且按原顺序回填 history。
- 真实 Bifrost 服务 `/agent/chat` 完整跑通本轮 prompt、tool schema、MCP resource、update_plan、set_title 和并发批链路。
- P1 工具链 E2E 仍通过。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：全面对齐 Codex 架构和风格，排除权限/沙箱/审批实现。
- 执行 `git status --short`、`git diff`。
- Review prompt、MCP resource、历史 shell alias 清理、design/human_tests。
- 运行 targeted Rust tests 与 agent P1 E2E。
- 修复失败或遗漏。

### 第 2 轮

- 再次复核第 1 轮后的 diff。
- 检查 resource tools 是否被错误 deferred，历史 shell alias 是否仍被错误暴露或执行。
- 复跑受影响测试。
- 确认无需第 3 轮或继续追加。

## 校验要求

- e2e-test 技能验证必须先于 rust-project-validate。
- 至少执行一次 `cargo test --workspace --all-features`；若环境阻塞必须在最终验证矩阵说明风险。
- 本轮不执行 local-ci 的前提只能是改动范围集中在 agent crate 且已执行 workspace all-features。

## 文档更新要求

- 本文件记录架构对齐方案。
- `human_tests/agent-codex-alignment.md` 记录真实场景测试。
- `human_tests/readme.md` 同步索引。

## Phase 2：Codex 风格事件驱动 Loop 与工具调度

用户指出的关键差异是：Codex 的 turn loop 以 Responses 流式 item/event 为中心，工具调用由 router/runtime 调度，并通过 `FuturesOrdered` 并发执行后按模型调用顺序回填 history；Bifrost 原先是 Chat Completions 同步 `response -> 顺序执行工具 -> 下一轮`。

本轮将 Bifrost 的内部执行模型改为 Codex 风格的 turn runtime：

- 新增 `turn_runtime` 模块，定义 `CodexTurnEventKind` / `CodexTurnEvent`，把 turn start、model request、model response、tool batch start/end、tool call start/end、deferred tool loaded、turn completed/stopped 变成可审计事件流。
- `AgentSession.last_turn_events` 保存最近一个 turn 的事件流，避免只能从日志猜测 runtime 过程。
- `session.rs` 工具执行从单个 `for tool_call` 改为批调度：
  - 本地无 session 路由副作用的工具进入 parallel batch。
  - parallel batch 使用 `FuturesOrdered` 并发 poll。
  - tool result 仍按 `response.tool_calls` 原顺序写入 `tool_calls_log` 和 Chat Completions history，避免 orphan tool message 或顺序漂移。
  - `tool_search` 成功后仍在当前顺序点加载 deferred tool definition。
  - MCP tools 暂保持顺序执行，因为当前 `McpManager::call_tool` 需要 `&mut self` 且 server 并发能力尚未建模。
  - `switch_workdir`、`set_title`、`update_plan`、`request_user_input`、Goal 工具保持顺序执行，因为它们会修改 session 状态、UI 计划、工作目录、可见工具集或交互状态。
- stop/cancel 语义保持兼容：并发批被取消时，从尚未回填 history 的 tool call 开始追加 cancelled tool result，保证后续模型请求不会看到不完整的 assistant tool_calls。

### Phase 2 不实现范围

- 不实现 Codex 的 Responses API wire format；Bifrost 外部模型请求仍可继续使用 Chat Completions。
- 不实现权限、沙箱、审批。
- 不把 MCP server tools 并发化，除非后续引入 per-server parallel 配置和 manager 内部并发安全路由。
