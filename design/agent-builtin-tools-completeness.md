# Agent Builtin Tools Completeness

## 功能模块说明

本模块用于把 Bifrost agent 的编程工具面与 `/Users/eden/work/github/codex` 中 Codex 的内置编程工具对齐。目标不是只补工具名，而是对齐模型可见 schema、运行时行为、错误边界、输出截断、长任务交互、真实验证文档。

## Codex 参照面

当前 Codex 的工具计划由 `codex-rs/tools/src/tool_registry_plan.rs` 组装，重要编程工具包括：

- 环境执行：`exec_command`、`write_stdin`，以及旧形态 `shell`/`shell_command`/`local_shell`
- 文件修改：`apply_patch`
- 进度与目标：`update_plan`、`get_goal`、`create_goal`、`update_goal`
- 本地视觉输入：`view_image`
- 用户交互与权限：`request_user_input`、`request_permissions`
- 工具发现：`tool_search`、`tool_suggest`/`request_plugin_install`
- MCP resource：`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`
- 子 Agent 协作：`spawn_agent`、`wait_agent`、`close_agent`、`send_input`/`resume_agent` 或 v2 的 `send_message`/`followup_task`/`list_agents`
- 可选扩展：`web_search`、`image_generation`、`code_mode`、`spawn_agents_on_csv`、`report_agent_job_result`

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

MCP 已有工具调用和 resources 的基础实现，但 resource 工具当前使用 `mcp__list_resources` 命名，和 Codex 的 canonical `list_mcp_resources` 仍不一致。

## 实现计划

### 第一批：编程核心工具

1. 新增 `exec_command`
   - 对齐 Codex schema 字段：`cmd`、`workdir`、`shell`、`tty`、`yield_time_ms`、`max_output_tokens`、`login`、`sandbox_permissions`、`justification`、`prefix_rule`
   - 默认工作目录为当前 turn cwd
   - 支持长任务：如果 `yield_time_ms` 内未结束，返回 `session_id`，后续由 `write_stdin` 继续轮询或输入
   - 输出包含 `chunk_id`、`wall_time_seconds`、`exit_code`、`session_id`、`original_token_count`、`output`
   - Bifrost 当前不实现 OS sandbox/approval，`sandbox_permissions` 等字段先作为兼容参数接受，并在输出中保持透明，不扩大权限

2. 扩展 `write_stdin`
   - 兼容 Codex `write_stdin` schema：`session_id`、`chars`、`yield_time_ms`、`max_output_tokens`
   - 保留旧 PTY `input` 字段作为向后兼容
   - 当 `session_id` 为数字时走 `exec_command` 会话；当为字符串 UUID 时继续走旧 `shell_pty` 会话

3. 新增 `view_image`
   - 接受本地图片路径与可选 `detail=original`
   - 验证路径存在且是文件，读取为 data URL，返回 `image_url` 与 `detail`
   - 限制支持常见图片后缀，避免任意大文件误读

4. 新增 `request_user_input`
   - 对齐 Codex schema 与参数校验
   - Bifrost 当前没有交互 UI 挂起/恢复通道；运行时以明确错误返回，避免模型误以为已经获得用户输入
   - 后续接入 IM Gateway/WebUI 时再把请求落到可交互队列

5. 新增 `tool_search`
   - 当前先检索本地已注册工具与 MCP direct tools
   - 返回匹配工具名、描述与 schema 摘要
   - 后续再扩展为 Codex 式 deferred dynamic tool loader

### 第二批：协作与外部能力

- `spawn_agent`/`wait_agent`/`close_agent`/`send_input`/`resume_agent` 需要独立的 AgentSession 管理、后台 turn 调度、消息队列、状态持久化和取消语义。禁止只返回假的 agent id。
- `request_permissions` 需要和 Bifrost 的本地/远程执行权限模型、Shell Access、File Access 或未来 sandbox 统一。
- `web_search` 和 `image_generation` 需要明确供应商、网络策略、密钥来源与输出事件协议，不能默认偷用普通 shell 网络。
- `code_mode` 需要独立 JS/TS 工具运行时和工具结果适配层，属于较大架构工作。

## 测试方案

### 单元测试

- `exec_command_returns_completed_output`：短命令在一次调用内返回 `exit_code=0` 和输出。
- `exec_command_yields_session_and_write_stdin_polls`：长命令先返回 `session_id`，随后 `write_stdin` 轮询到最终输出。
- `write_stdin_accepts_chars_and_legacy_input`：同时兼容 Codex `chars` 与旧 `input` 字段。
- `view_image_rejects_missing_file`：缺失路径返回明确错误。
- `view_image_returns_data_url`：PNG/JPEG/GIF/WebP 返回 data URL。
- `request_user_input_validates_questions`：空 options 或超过 3 个问题返回错误。
- `tool_search_finds_registered_tool`：查询 `exec` 能找到 `exec_command`。

### E2E 测试

- 扩展 `crates/agent/tests/p1_tools_e2e.rs`
  - `exec_command_tool_works_end_to_end`
  - `view_image_tool_works_end_to_end`
  - `tool_search_lists_core_tools`

### 真实场景测试

- 新增 `human_tests/agent-builtin-tools-completeness.md`
- 更新 `human_tests/readme.md`
- 按用例真实执行：
  - `exec_command` 短命令
  - `exec_command` 长命令 + `write_stdin` 轮询
  - `view_image` 本地图片 data URL
  - `request_user_input` 当前不可交互错误
  - `tool_search` 搜索新增工具

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
