# Agent Codex Alignment

## 背景

`~/work/github/codex` 是当前对齐目标：Codex 用 Responses API 上的 item/event 流作为核心 runtime，工具由 router / runtime 调度，本地并发工具批量 `FuturesOrdered` 执行并按调用顺序回填 history；MCP resources 作为核心工具直接注册；shell 相关工具收敛到 `exec_command` / `write_stdin`；权限 / OS sandbox / 审批由平台层实现。

Bifrost Agent 早期是 Chat Completions 顺序 loop：一个 response 拿到后按顺序执行 tool_calls，`update_plan` / `set_title` / MCP resource 等工具的命名、暴露路径、schema 与 Codex 有偏移；`shell` / `shell_pty` / `shell_command` / `local_shell` 等历史 alias 与统一终端工具共存，扩大了模型可见工具面。

本模块的目标不是照搬 Responses API wire format，而是在 Bifrost 保留 Chat Completions 前提下，把 prompt 分层、工具命名、暴露策略、上下文边界、错误提示与执行调度尽量与 Codex 一致。本轮显式排除本地权限升级、OS sandbox、审批工作流：Bifrost 暂不实现这些能力，相关字段不进入工具 schema，也不写进默认 prompt。

## 用户目标验证清单

### 必须实现

- Prompt 风格对齐：默认指令只描述当前 Agent 行为，不写内部兼容说明。
- MCP resource 工具命名对齐 Codex canonical 名：
  - `list_mcp_resources`
  - `list_mcp_resource_templates`
  - `read_mcp_resource`
- 存在 MCP server 连接时，resource 工具直接可见（direct exposure），不进入 deferred `tool_search`。
- Terminal 工具面收敛：只保留 `exec_command` / `write_stdin`，删除 `shell` / `shell_pty` / `shell_command` / `local_shell` 旧入口。
- MCP resource 执行走 MCP `resources/list`、`resources/templates/list`、`resources/read`，输出包含 server 字段。
- Loop 架构对齐：turn runtime 记录 Codex 风格 event stream（`CodexTurnEvent`），不再只依赖同步 Chat Completions loop 推断过程。
- 工具调度对齐：本地可并发工具使用 `FuturesOrdered` 批量执行，tool result 按 `response.tool_calls` 原顺序落回 history；状态型工具保持顺序执行。

### 必须不破坏

- 不改变 `exec_command`、`write_stdin`、`apply_patch` 的可见顺序与既有行为。
- 不把权限、沙箱、审批伪实现成空成功；相关能力仍然不支持。
- 不让 MCP server tool 数量阈值改变：server tools 仍按 `DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD >= 100` 进入 deferred loading。
- 历史 shell alias 不再可见也不再可执行；避免扩大隐藏工具面。
- 并发工具结果不得按完成时间写入 history；必须按模型调用顺序，避免 Chat Completions tool message 序列损坏。
- 不并发化 `tool_search`、Goal、`update_plan`、`set_title`、`switch_workdir`、`request_user_input` 等状态型工具。

### 必须真实验证

- Rust 单测覆盖 MCP resource schema 名称、`additionalProperties=false`、`ToolDefinition` 转换。
- Rust 单测覆盖 `shell` / `shell_pty` / `shell_command` / `local_shell` 被拒绝而不是被映射。
- Rust 单测覆盖并发本地工具不互相阻塞，history 按 tool_call 原顺序回填。
- Rust 单测覆盖状态型工具保持 ordered。
- Agent P1 工具链 E2E 仍通过。
- `test_agent_codex_alignment_chat_api.sh` 真实链路 E2E 通过。
- human_tests 覆盖默认 prompt、MCP resource canonical、历史 shell alias 已移除、并发批调度与事件流。

### 必须交付

- 更新本设计文档。
- 更新 `human_tests/agent-codex-alignment.md` 与 `human_tests/readme.md` 索引。
- 至少两轮 Review/Fix/Test。
- 先执行 e2e-test 技能范围验证，再执行 rust-project-validate。

## 产品语义

### Prompt 分层

Bifrost 仍保持 Chat Completions prompt 分层：

- `system`：base instructions。
- `developer`：配置级 developer instructions 与 skill metadata。
- `user`：用户自定义 instructions 与环境上下文。

本轮不在 base instructions 里加入兼容说明。默认 prompt 只保留面向 agent 的工作契约；权限、沙箱、审批这些暂不支持的字段不进入工具 schema。

### MCP resource 工具

Codex 把 MCP resources 作为核心工具注册；Bifrost 早期使用 `mcp__list_resources` 等非 canonical 名称，且没有把它们暴露到 turn 工具面。本轮改为：

- Resource tool definition 使用 canonical 名称与 Codex 风格描述。
- 当 MCP manager 有连接时，resource tools 直接加入 `McpToolExposure.direct_tools`。
- Server tools 仍独立按 `DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD = 100` 决定 direct/deferred。
- `McpManager::call_tool` 对 resource tools 做虚拟路由，不依赖 `tool_routing`。
- Resource read 结果携带 server 字段，便于模型后续 `read_mcp_resource`。

### Terminal 工具面

只保留统一终端协议 `exec_command` / `write_stdin`。历史 `shell` / `shell_pty` / `shell_command` / `local_shell` 不再注册；模型若继续调用旧名会得到 `unknown tool`，暴露旧协议残留。

### Codex 风格 turn runtime

新增 `turn_runtime` 模块，定义 `CodexTurnEventKind` / `CodexTurnEvent`：

- `TurnStarted`
- `ModelRequestPrepared`
- `ModelResponseReceived`
- `ToolBatchStarted` / `ToolBatchFinished`
- `ToolCallStarted` / `ToolCallFinished`
- `DeferredToolLoaded`
- `TurnStopped` / `TurnCompleted`

`AgentSession.last_turn_events` 保存最近一个 turn 的事件流，便于诊断、审计与 human_tests 复核。

## 技术细节

### 关键类型与常量

```rust
pub const DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD: usize = 100;

pub struct McpToolExposure {
    pub direct_tools: Vec<ToolDefinition>,
    pub deferred_tools: Vec<ToolDefinition>,
}

pub enum CodexTurnEventKind {
    TurnStarted,
    ModelRequestPrepared,
    ModelResponseReceived,
    ToolBatchStarted { size: usize, ordered: bool },
    ToolBatchFinished,
    ToolCallStarted { name: String, call_id: String },
    ToolCallFinished { name: String, call_id: String, success: bool },
    DeferredToolLoaded { name: String },
    TurnStopped { reason: String },
    TurnCompleted,
}
```

`turn_runtime::classify(tool)` 决定工具是 parallel 还是 ordered。

### 并发调度

- 本轮每个 assistant response 中的 tool_calls 会先按分类拆成若干 batch：
  - 连续的 parallel 工具进入一个 batch，使用 `FuturesOrdered` 并发 poll。
  - 一旦遇到 ordered 工具（`switch_workdir`、`set_title`、`update_plan`、`request_user_input`、Goal 系列、`send_msg`、`schedule_*` 等），先 flush 前面的 parallel batch，然后单独顺序执行 ordered 工具。
- 每个 tool result 按 `response.tool_calls` 原顺序写入 `tool_calls_log` 与 Chat Completions history。
- stop/cancel 语义：并发批被取消时，从尚未回填 history 的 tool call 开始追加 cancelled tool result，保证后续模型请求不会看到不完整的 assistant tool_calls。

### 关键文件

- `crates/agent/src/prompts/base_instructions/default.md`
- `crates/agent/src/tools/mod.rs`
- `crates/agent/src/turn_runtime.rs`（新模块）
- `crates/agent/src/session.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/mcp/resources.rs`
- `crates/agent/src/mcp/mod.rs`
- `crates/agent/src/mcp/mod_tests.rs`
- `crates/agent/tests/p1_tools_e2e.rs`
- `e2e-tests/tests/test_agent_codex_alignment_chat_api.sh`
- `human_tests/agent-codex-alignment.md`
- `human_tests/mcp-elicitation-resources.md`
- `human_tests/readme.md`

## CLI 交互

本模块不新增 CLI 命令。`bifrost agent` 相关命令保持不变；模型运行时行为通过 API 侧生效。

## Web UI 交互

- Agent Chat 工具面板：显示的工具名称与 canonical Codex 名一致（`list_mcp_resources`、`read_mcp_resource` 等），不再出现 `shell_command` / `local_shell`。
- MCP resource 工具在 MCP 已连接时始终可见；未连接时隐藏。
- `tool_search` 面板保留，只作为 deferred MCP server tools 的入口。

## Admin API

本模块不新增 admin API endpoint；改动集中在 `/api/agent/chat/*` 的模型请求构造、tool schema 序列化与执行调度。

## Sync / 导入导出 / 分享边界

本模块只影响 agent runtime；不涉及 rule sync、group、share URL。

## 实现切分

### Phase 1：MCP resource canonical & 工具面收敛

- 重命名 resource helper 到 canonical 名称。
- 把 resource tools 加入 `direct_tools`。
- 删除 `shell` / `shell_pty` / `shell_command` / `local_shell` 注册与 alias。
- 单元测试覆盖 schema 与 alias 拒绝。

### Phase 2：Codex 风格 turn runtime 与并发调度

- 新增 `turn_runtime` 模块与 `CodexTurnEvent` 事件流。
- `AgentSession.last_turn_events` 保存最近 turn 事件。
- 工具执行改为 batch 调度：parallel batch 使用 `FuturesOrdered`；ordered 工具单独顺序执行。
- history 按 tool_call 原顺序回填。
- stop/cancel 从未回填的 tool call 起追加 cancelled result。

### Phase 3：真实链路 E2E 与 human_tests

- 新增 `test_agent_codex_alignment_chat_api.sh`，覆盖默认 prompt、canonical MCP resource、deferred MCP tool、`update_plan`、`set_title`、MCP resource read、并发 `exec_command` 批与 history 顺序。
- 新增 / 更新 `human_tests/agent-codex-alignment.md` 与 `human_tests/mcp-elicitation-resources.md`。

### Phase 4：Prompt 与文档清理

- 修剪 base instructions 中的兼容说明。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- `mcp::resources::tests::test_list_resources_tool_def_structure`
- `mcp::resources::tests::test_list_resource_templates_tool_def_structure`
- `mcp::resources::tests::test_read_resource_tool_def_structure`
- `mcp::resources::tests::test_all_resource_tool_definitions_are_function_tools`
- `mcp::tests::mcp_resource_tools_stay_direct_when_server_tools_are_deferred`
- `tools::tests::unknown_shell_aliases_are_rejected`
- `p1_tools_e2e::legacy_shell_tools_are_not_registered_by_default`
- `turn_runtime::tests::side_effect_tools_are_ordered`
- `turn_runtime::tests::ordinary_local_tools_can_run_in_parallel`
- `session::tests::codex_parallel_tool_batch_preserves_history_order`
- `session::tests::codex_turn_events_are_recorded_for_last_turn`
- `session::tests::cancelled_parallel_batch_fills_missing_tool_results`

### E2E 测试

- `cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end`
- `cargo test -p bifrost-agent --test p1_tools_e2e tool_search_is_hidden_without_deferred_tools`
- `cargo test -p bifrost-agent --test p1_tools_e2e tool_search_returns_deferred_mcp_tools`
- `bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh`：真实链路验证——编译 `bifrost`，用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy` 启动服务，配置 mock Chat Completions provider 与 stdio MCP fixture，通过真实 `/agent/chat` 覆盖默认 prompt、MCP resource canonical tools、deferred MCP tools 通过 `tool_search` 发现但不直接暴露、`update_plan`、`set_title`、MCP resource read、并发 `exec_command` tool batch 与 history 顺序。

### 真实场景测试 human_tests

- `human_tests/agent-codex-alignment.md`：
  - 默认 prompt 不包含权限、沙箱、审批兼容说明。
  - 默认工具列表不包含 `shell_command` / `local_shell`，旧 alias 执行被拒绝。
  - MCP resource schema 使用 canonical 名称。
  - Agent loop 记录 Codex 风格 turn events。
  - 本地可并发工具通过 `FuturesOrdered` 并发执行且按原顺序回填 history。
  - 真实 `/agent/chat` 跑通本轮 prompt、tool schema、MCP resource、`update_plan`、`set_title` 和并发批链路。
  - P1 工具链 E2E 仍通过。
- `human_tests/mcp-elicitation-resources.md`：MCP elicitation 与 resource 交互沿用 canonical 名称。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo test -p bifrost-agent mcp::resources::tests`
- `cargo test -p bifrost-agent turn_runtime::tests`
- `cargo test -p bifrost-agent session::tests::codex_parallel_tool_batch_preserves_history_order`
- `cargo test -p bifrost-agent --test p1_tools_e2e`
- `bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh`
- 至少执行一次 `cargo test --workspace --all-features`。
- 收尾按项目规则执行 `rust-project-validate`。
- 本机 no-local-coverage 约定生效时不执行 `make coverage`；交付说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：全面对齐 Codex 架构与风格，排除权限/沙箱/审批实现。
- 执行 `git status --short` / `git diff`。
- Review prompt、MCP resource、历史 shell alias 清理、并发调度、history 顺序、design/human_tests。
- 运行 targeted Rust tests 与 agent P1 E2E。
- 修复失败或遗漏。

### 第 2 轮

- 再次复核第 1 轮后的 diff。
- 重点检查：resource tools 是否被错误 deferred；历史 shell alias 是否仍被错误暴露或执行；并发批 cancel 边界；`last_turn_events` 是否稳定。
- 复跑受影响测试。
- 确认无需第 3 轮或继续追加。

## Phase 2：Codex 风格事件驱动 Loop 与工具调度（保留章节）

用户指出的关键差异是：Codex 的 turn loop 以 Responses 流式 item/event 为中心，工具调用由 router/runtime 调度，并通过 `FuturesOrdered` 并发执行后按模型调用顺序回填 history；Bifrost 原先是 Chat Completions 同步 `response -> 顺序执行工具 -> 下一轮`。

本轮把 Bifrost 内部执行模型改为 Codex 风格的 turn runtime：

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

- 不实现 Codex 的 Responses API wire format；Bifrost 外部模型请求仍继续使用 Chat Completions。
- 不实现权限、沙箱、审批。
- 不并发化 MCP server tools，除非后续引入 per-server parallel 配置和 manager 内部并发安全路由。

## 风险与决策点

- 是否把 `bash e2e-tests/tests/test_agent_codex_alignment_chat_api.sh` 加到 CI shell E2E sharding：建议加入以防回归。
- MCP server tools 并发化需要 `McpManager` 内部锁重构，本轮不做。
- 历史 shell alias 用户脚本迁移：默认拒绝 + 明确错误提示；如出现大范围回退请求，再考虑短期 alias warning 通道。
- `last_turn_events` 大小无上限时可能吃内存：当前只保存最近 1 个 turn；如果后续需要审计多 turn，应加入 ring buffer 或落盘。
