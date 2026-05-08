# Agent Builtin Tools Completeness

## 功能模块说明

本模块用于完善 Bifrost agent 的编程工具面。目标不是只补工具名，而是完善模型可见 schema、运行时行为、错误边界、输出截断、长任务交互、真实验证文档。

## 参照工具面

编程工具标准包含以下重要工具：

- 环境执行：`exec_command`、`write_stdin`，以及旧形态 `shell`/`shell_command`/`local_shell`
- 文件修改：`apply_patch`
- 进度与目标：`update_plan`、`get_goal`、`create_goal`、`update_goal`
- 本地视觉输入：`view_image`
- 用户交互与权限：`request_user_input`、`request_permissions`
- 工具发现：`tool_search`、`tool_suggest`/`request_plugin_install`
- MCP resource：`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`
- 子 Agent 协作：`spawn_agent`、`wait_agent`、`close_agent`、`send_input`/`resume_agent` 或 v2 的 `send_message`/`followup_task`/`list_agents`
- 可选扩展：`web_search`、`image_generation`、`code_mode`、`spawn_agents_on_csv`、`report_agent_job_result`

### `tool_search` 对齐结论

按标准逐项核对，`tool_search` 不是普通常驻本地工具，而是 deferred tool discovery 的模型可见入口：

| 行为要求 | Bifrost 对齐方式 |
| --- | --- |
| `DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD` 固定为 `100` | `crates/agent/src/mcp/mod.rs` 使用同名阈值 `100` |
| 仅在启用强制 defer 或 deferred MCP 数量 `>= 100` 时进入 deferred loading | Bifrost 当前没有 feature flag/config gate，默认启用数量阈值；MCP 工具数 `>= 100` 时 deferred |
| 不 defer 时，候选 MCP 全部直接暴露，`deferred_tools=None` | Bifrost 低于 100 个 MCP 工具时全部直接加入 `tools`，不暴露 `tool_search` |
| defer 时只保留显式启用的 App 工具直暴，其余 deferred；空 deferred 不返回 | Bifrost 暂无 App connector，因此 defer 时 MCP 工具全部放入 deferred；若为空则不暴露 `tool_search` |
| dynamic tools 只有 `defer_loading=true` 且 namespace 条件满足才进 deferred | Bifrost 暂无 dynamic tools 与 Responses API namespace，第一阶段只实现 MCP deferred `ToolDefinition` |
| 只有存在 deferred MCP 或 deferred dynamic tools 时才注册 `tool_search` | Bifrost `ToolRegistry::with_defaults` 不再注册 `tool_search`；`AgentSession` 仅在 `tool_search_entries` 非空时加入 `tool_search` |
| deferred specs 的 handler 仍注册，虽然 spec 初始不可见 | Bifrost MCP manager 始终保留所有 MCP 工具定义与调用路由；搜索加载后同一 turn 后续模型调用可调用 |
| schema 为 `{query, limit}`，`query` 必填，`additionalProperties=false` | Bifrost `tool_search` schema 对齐 `{query, limit}`、必填与额外字段拒绝，描述同样说明 deferred metadata/BM25/next model call |
| 搜索结果输出可注入下一次工具列表的 loadable specs | Bifrost 使用 Chat Completions `ToolDefinition` 作为可加载输出，并在 Agent loop 中追加到下一次请求的 `tools` |
| handler 基于 entries 的 `search_text` 构建 BM25 index | Bifrost `ToolSearchTool` 同样使用 `bm25` crate 对 deferred entries 建索引 |
| 空 query 与 `limit=0` 返回可给模型看的错误；默认 limit 为 8 | Bifrost 同步校验空 query、`limit=0`，默认 limit 为 8 |
| deferred entries 为空时返回空 `tools` | Bifrost 运行时为空时不会暴露 `tool_search`；直接构造空 handler 时也返回空结果 |
| BM25 搜索后输出 coalesced loadable specs；按 bucket 限制 | Bifrost 目前无 computer-use MCP 特例和 namespace 合并；BM25 结果直接返回 `ToolDefinition`，并为下划线工具名补了精确子串兜底 |
| entry 从排序后的 MCP 与 dynamic tools 构建 | Bifrost entry 从 MCP exposure 的 deferred tools 构建；dynamic tools 待引入后复用同一入口 |
| deferred specs 初始不暴露，被 `tool_search` 命中后再加载到后续 model request | Bifrost `session.rs` 在 `tool_search` 成功后解析 `{"tools":[...]}` 并追加到同一 turn 的后续模型请求 |

## 当前 Bifrost 状态

Bifrost `crates/agent/src/tools/mod.rs` 当前已有：

- `shell`
- `write_file`
- `read_file`
- `list_directory`
- `switch_workdir`
- `update_plan`
- `set_title`
- `apply_patch`
- `shell_pty`
- `write_stdin`
- `get_goal` / `create_goal` / `update_goal`

MCP 已有工具调用和 resources 的基础实现，但 resource 工具当前使用 `mcp__list_resources` 命名，和标准的 canonical `list_mcp_resources` 仍不一致。

## 实现计划

### 第一批：编程核心工具

1. 新增 `exec_command`
   - 对齐标准 schema 字段：`cmd`、`workdir`、`shell`、`tty`、`yield_time_ms`、`max_output_tokens`、`login`、`sandbox_permissions`、`justification`、`prefix_rule`
   - 默认工作目录为当前 turn cwd
   - 支持长任务：如果 `yield_time_ms` 内未结束，返回 `session_id`，后续由 `write_stdin` 继续轮询或输入
   - 输出包含 `chunk_id`、`wall_time_seconds`、`exit_code`、`session_id`、`original_token_count`、`output`
   - Bifrost 当前不实现 OS sandbox/approval，`sandbox_permissions` 等字段先作为兼容参数接受，并在输出中保持透明，不扩大权限

2. 扩展 `write_stdin`
   - 兼容标准 `write_stdin` schema：`session_id`、`chars`、`yield_time_ms`、`max_output_tokens`
   - 保留旧 PTY `input` 字段作为向后兼容
   - 当 `session_id` 为数字时走 `exec_command` 会话；当为字符串 UUID 时继续走旧 `shell_pty` 会话

3. 新增 `view_image`
   - 接受本地图片路径与可选 `detail=original`
   - 验证路径存在且是文件，读取为 data URL，返回 `image_url` 与 `detail`
   - 限制支持常见图片后缀，避免任意大文件误读

4. 新增 `request_user_input`
   - 对齐标准 schema 与参数校验
   - Bifrost 当前没有交互 UI 挂起/恢复通道；运行时以明确错误返回，避免模型误以为已经获得用户输入
   - 后续接入 IM Gateway/WebUI 时再把请求落到可交互队列

5. `tool_search` 暴露时机与 deferred loading
   - `tool_search` 不作为默认本地工具常驻暴露；只有存在 deferred tools 时才加入当前 turn 的模型可见工具列表。
   - MCP 工具数量阈值：`>= 100` 时启用 deferred loading；低于 100 时 MCP 工具直接暴露，不暴露 `tool_search`。
   - 当前 Bifrost 还没有 App connector / dynamic tool registry，第一阶段先覆盖 MCP deferred tools；dynamic tool 后续接入时按 `defer_loading: true` 直接触发。
   - `tool_search` 输出 `{"tools":[...]}`，其中每项是可加入下一次模型请求的 `ToolDefinition`；Agent loop 在同一 turn 后续模型调用前把返回的工具定义加入 `tools` 参数，允许模型继续调用刚搜索出的 MCP 工具。
   - 搜索默认 limit 为 8，空 query 和 `limit=0` 明确返回错误。

### 第二批：协作与外部能力

- `spawn_agent`/`wait_agent`/`close_agent`/`send_input`/`resume_agent` 需要独立的 AgentSession 管理、后台 turn 调度、消息队列、状态持久化和取消语义。禁止只返回假的 agent id。
- `request_permissions` 需要和 Bifrost 的本地/远程执行权限模型、Shell Access、File Access 或未来 sandbox 统一。
- `web_search` 和 `image_generation` 需要明确供应商、网络策略、密钥来源与输出事件协议，不能默认偷用普通 shell 网络。
- `code_mode` 需要独立 JS/TS 工具运行时和工具结果适配层，属于较大架构工作。

## 测试方案

### 单元测试

- `exec_command_returns_completed_output`：短命令在一次调用内返回 `exit_code=0` 和输出。
- `exec_command_yields_session_and_write_stdin_polls`：长命令先返回 `session_id`，随后 `write_stdin` 轮询到最终输出。
- `write_stdin_accepts_chars_and_legacy_input`：同时兼容 `chars` 与旧 `input` 字段。
- `view_image_rejects_missing_file`：缺失路径返回明确错误。
- `view_image_returns_data_url`：PNG/JPEG/GIF/WebP 返回 data URL。
- `request_user_input_validates_questions`：空 options 或超过 3 个问题返回错误。
- `tool_search_hidden_without_deferred_tools`：默认本地工具不包含 `tool_search`。
- `tool_search_returns_loadable_deferred_tool_definitions`：查询 deferred MCP 工具返回可加载 `ToolDefinition`。
- `mcp_tool_exposure_defers_at_threshold`：MCP 工具数等于 100 时触发 deferred loading。

### E2E 测试

- 扩展 `crates/agent/tests/p1_tools_e2e.rs`
  - `exec_command_tool_works_end_to_end`
  - `view_image_tool_works_end_to_end`
  - `tool_search_is_hidden_without_deferred_tools`
  - `tool_search_returns_deferred_mcp_tools`

### 真实场景测试

- 新增 `human_tests/agent-builtin-tools-completeness.md`
- 更新 `human_tests/readme.md`
- 按用例真实执行：
  - `exec_command` 短命令
  - `exec_command` 长命令 + `write_stdin` 轮询
  - `view_image` 本地图片 data URL
  - `request_user_input` 当前不可交互错误
  - `tool_search` 默认隐藏、MCP deferred 时暴露并加载搜索结果

## 校验要求

- `cargo test -p bifrost-agent exec_command`
- `cargo test -p bifrost-agent view_image`
- `cargo test -p bifrost-agent tool_search`
- `cargo test -p bifrost-agent --test p1_tools_e2e`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-06 covers the workspace all-features compile gate for agent message serialization types.
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-07 covers a real Bifrost server on a non-production port and a live `/api/im-gateway/agent/chat` turn that asks the model to use the new tools.
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-08 covers the final local CI static gate, including IM gateway compile regressions surfaced while validating the tool work.
- 最后按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`
