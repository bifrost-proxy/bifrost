# Agent Builtin Tools Completeness

## 功能模块说明

本模块用于完善 Bifrost agent 的编程工具面。目标不是只补工具名，而是完善模型可见 schema、运行时行为、错误边界、输出截断、长任务交互、真实验证文档。

## 参照工具面

编程工具标准包含以下重要工具：

- 环境执行：模型可见主路径只保留 `exec_command`、`write_stdin`；旧形态 `shell`/`shell_pty`/`shell_command`/`local_shell` 不再保留实现或默认入口
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

- `exec_command`
- `write_file`
- `read_file`
- `list_directory`
- `switch_workdir`
- `update_plan`
- `set_title`
- `apply_patch`
- `write_stdin`
- `get_goal` / `create_goal` / `update_goal`

MCP 已有工具调用和 resources 的基础实现；resource 工具已收敛到 Codex canonical 名称 `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`，并在存在 MCP server 时作为 direct core tools 暴露，不参与 server tool 的 deferred threshold。

## 实现计划

### 第一批：编程核心工具

1. 新增 `exec_command`
   - 对齐标准 schema 字段（排除 Bifrost 当前不存在的权限/沙箱模块）：`cmd`、`workdir`、`shell`、`login`、`tty`、`yield_time_ms`、`max_output_tokens`
   - 默认工作目录为当前 turn cwd
   - 支持长任务：如果 `yield_time_ms` 内未结束，返回 `session_id`，后续由 `write_stdin` 继续轮询或输入
   - 最终实现必须按真实 child process 生命周期收敛：`exec_command` 直接 spawn pipe/PTY 子进程，后台保留 child handle、stdin writer、stdout/stderr capped buffer 和 exit 状态；完成判断来自 `try_wait()` / PTY child status，而不是 shell prompt、stdout sentinel 或模型推测
   - 输出包含 `chunk_id`、`wall_time_seconds`、`exit_code`、`session_id`、`original_token_count`、`output`；是否仍在运行由 `session_id` 非空且 `exit_code` 为空表达，不再额外定义第二套 `running` 字段
   - 短命令在 yield window 内退出时返回 `exit_code` 且不保留 session；长命令在首次 yield 后继续运行，最终一次 `write_stdin` 轮询必须返回真实 `exit_code` 并清理 session
   - pipe 模式支持 stdin forwarding；PTY 模式支持真实 TTY、交互式程序；`write_stdin` 收到 Ctrl-C 时必须终止并清理后台 session，工具结果按主动取消成功处理并保留真实 exit code
   - Bifrost 当前不实现 OS sandbox/approval，因此不把 `sandbox_permissions`、`justification`、`prefix_rule` 暴露为伪协议；后续如引入权限模块再整体接入

2. 扩展 `write_stdin`
   - 兼容标准 `write_stdin` schema：`session_id`、`chars`、`yield_time_ms`、`max_output_tokens`
   - 模型可见协议和运行时解析都只接受数字 `session_id`；旧字符串 session id 和隐藏 `input` 字段必须拒绝，避免形成第二套终端协议
   - 只查找 `exec_command` 真实进程会话；代码中不再保留 `shell_pty` session manager 或 fallback
   - 空输入轮询不得先清空旧 buffer，避免长命令在两次 poll 之间完成时丢失输出；如果本次写入了新 stdin，则等待写入后新增字节或进程结束再返回

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

### 第三批：真实 PTY 与远端交互输入

1. 本地 `exec_command tty=true`
   - `exec_command` 在 `tty=true` 时不再复用普通 pipe 后端，而是通过 `portable-pty` 创建真实 PTY。
   - PTY session 仍复用现有 `write_stdin` 协议；`exec_command` 返回 `session_id` 后，`write_stdin` 负责后续输入、轮询输出和 Ctrl-C 等控制字符转发。
   - PTY 默认尺寸为 `80x24`、`TERM=xterm-256color`，工作目录和 shell 选择规则沿用原 `exec_command` 参数。
   - `tty=false` 继续走原 pipe session，避免影响非交互命令的 stdout/stderr 拆分和 completion 语义。

2. Codex CLI 交互回归
   - `codex --help` 只能验证命令存在，不能证明 agent loop 可以驱动交互式程序。
   - 新增真实回归用例启动本机 Codex CLI 的 interactive 入口，在真实 PTY 中观察欢迎/登录/界面输出，并通过 `write_stdin` 发送 Ctrl-C 清理会话。
   - 新增黑盒真实模型回归：启动当前源码构建的 Bifrost，配置真实 `aidp_crawl` 模型 provider，经 `/agent/chat` 让模型自行调用 `exec_command tty=true` 启动交互 Python、调用 `write_stdin` 写入引导文本，再启动 `codex --sandbox read-only .` interactive session 并通过 `write_stdin` 追加引导问题。
   - 该测试标记为 ignored，避免 CI 因机器未安装或未登录 Codex CLI 失败；human_tests 与 E2E 脚本在本地有 `codex` 时会主动尝试运行。

3. 终端工具形态收敛策略
   - 真实默认数据目录回归显示，用户说“启动 codex cli，新建一个任务/给 Codex 派发任务”时，模型选择 blocking `shell` 执行 `codex exec review`，导致没有 `session_id`、无法用 `write_stdin` 继续观察/输入/取消。这个问题的本质不是 Codex 特例，而是没有把命令分成短命令、长任务、交互任务、等待 stdin 的前台任务。
   - 对照 `~/work/github/codex` 后，关键经验是启用 unified exec 时模型可见工具收敛为 `exec_command` + `write_stdin`；在 Bifrost 当前无兼容压力的前提下，旧 shell 类 handler 应直接删除，可靠性来自单一路径，而不是并行维护 `shell_pty` 与 `exec_command` 两套模型协议。
   - 默认 base instructions 必须明确：凡是需要执行 shell/terminal 命令，都统一使用 `exec_command`，保存返回的 `session_id`，后续通过 `write_stdin` 观察、追加输入或 Ctrl-C 清理；TUI/readline/交互式终端程序设置 `tty=true`。
   - Bifrost 源码中历史 `shell` 与 `shell_pty` handler 已删除，`write_stdin` 与 `exec_command` 同文件实现并共享同一个 `ExecSessionManager`；模型可见终端入口只保留 `exec_command` / `write_stdin`。

3. Remote Invoke stdin forwarding
   - caller 侧 `remote command exec --stdin/--pty/--interactive` 在 command envelope 中携带 `stdin_mode`、`pty`（含启动时终端 `rows/cols`）与 `output_mode`。
   - caller CLI 对 interactive 模式开启 raw mode，把本地 stdin 字节加密为 caller-to-client `call_frame`，通过 relay `/calls/{call_id}/input` 发送。
   - interactive 模式默认跳过普通 streaming Done digest 校验；`pty_merged` 终端字节流可能经 legacy exit 收敛，不能把缺失 Done digest 误判为命令失败。
   - target worker 在 active call map 中登记 stdin channel，收到 caller-to-client `call_frame` 后解密并转发到 executor。
   - executor 对允许 stdin 的 shell command 打开 child stdin 并持续写入收到的字节。
   - 真实 remote E2E 在本地 PTY 中启动 `bifrost remote exec --interactive --shell-text ...`，通过 relay/target/caller 全链路把输入行传给远端 shell 程序，并在 Recent Calls 中确认命令落库。
   - 取消仍走既有 call cancel 通道；raw mode guard 负责退出时恢复终端。当前先同步启动时终端尺寸；远端运行期 resize frame 和远端真 PTY resize 作为后续增强继续落在 remote interactive 完整化工作中。

### 第二批：协作与外部能力

- `spawn_agent`/`wait_agent`/`close_agent`/`send_input`/`resume_agent` 需要独立的 AgentSession 管理、后台 turn 调度、消息队列、状态持久化和取消语义。禁止只返回假的 agent id。
- `request_permissions` 需要和 Bifrost 的本地/远程执行权限模型、Shell Access、File Access 或未来 sandbox 统一。
- `web_search` 和 `image_generation` 需要明确供应商、网络策略、密钥来源与输出事件协议，不能默认偷用普通 shell 网络。
- `code_mode` 需要独立 JS/TS 工具运行时和工具结果适配层，属于较大架构工作。

## 测试方案

### 单元测试

- `exec_command_returns_completed_output`：短命令在一次调用内返回 `exit_code=0` 和输出。
- `exec_command_yields_session_and_write_stdin_polls_to_exit`：长命令先返回数字 `session_id` 且 `exit_code=null`，随后 `write_stdin` 轮询到最终 `exit_code=0` 与末尾输出，并确认 session 被清理。
- `exec_command_write_stdin_drives_pipe_process`：pipe 模式启动等待 stdin 的 Python 进程，`write_stdin` 写入后返回真实输出与最终 exit code。
- `exec_command_ctrl_c_terminates_running_process`：长时间运行的 pipe 进程收到 `write_stdin` 的 Ctrl-C 后被终止并清理 session，避免后台进程泄漏。
- `test_exec_command_tty_reports_isatty_true`：`tty=true` 启动 `python3 -c 'import os,sys; print(os.isatty(0), os.isatty(1))'`，必须输出 `True True`。
- `write_stdin_rejects_legacy_protocol_fields`：`write_stdin` 拒绝旧字符串 session id 和隐藏 `input` 字段，确保运行时协议和 schema 一致。
- `test_default_base_instructions_describe_terminal_tool_selection`：默认 base instructions 必须包含统一使用 `exec_command`、`tty=true` 与 `write_stdin` 的通用规则，并且不再出现 `shell`/`shell_pty` 推荐。
- `model_visible_tool_definitions_prefer_unified_exec_tools`：模型可见工具列表中包含 `exec_command`、`write_stdin`，且不包含 legacy `shell`/`shell_pty`。
- `terminal_tools_reject_non_schema_legacy_arguments`：`exec_command` 拒绝未暴露的权限/沙箱字段，`write_stdin` 拒绝隐藏 `input` 与旧字符串 session id。
- `view_image_rejects_missing_file`：缺失路径返回明确错误。
- `view_image_returns_data_url`：PNG/JPEG/GIF/WebP 返回 data URL。
- `request_user_input_validates_questions`：空 options 或超过 3 个问题返回错误。
- `tool_search_hidden_without_deferred_tools`：默认本地工具不包含 `tool_search`。
- `tool_search_returns_loadable_deferred_tool_definitions`：查询 deferred MCP 工具返回可加载 `ToolDefinition`。
- `mcp_tool_exposure_defers_at_threshold`：MCP 工具数等于 100 时触发 deferred loading。

### E2E 测试

- 扩展 `crates/agent/tests/p1_tools_e2e.rs`
  - `exec_command_tool_works_end_to_end`
    - 覆盖短命令一次性完成、长命令 yield session 后通过 `write_stdin` 轮询到最终 exit code、TTY 交互命令 stdin/stdout 均为 TTY、stdin 写入回显
  - `codex_cli_interactive_starts_in_real_pty`：本地安装 Codex CLI 时启动真实 interactive session，并用 `write_stdin` 发送 Ctrl-C 清理。
  - `view_image_tool_works_end_to_end`
  - `tool_search_is_hidden_without_deferred_tools`
  - `tool_search_returns_deferred_mcp_tools`
- 新增 terminal E2E：
  - 本轮一次性临时 harness：执行 PTY isatty 单测、`exec_command + write_stdin` P1 E2E，并在本地存在 `codex` 时主动运行 Codex interactive ignored 回归。该 harness 仅用于本次验证，不落库。
  - 本轮一次性临时 harness：启动真实 Bifrost，配置真实模型 provider，通过 `/agent/chat` 让模型实际调用 `exec_command tty=true`、`write_stdin`、Codex CLI interactive session 和追加引导问题；不允许降级到 mock provider。该 harness 仅用于本次验证，不落库。
  - 本轮一次性临时 harness：启动真实 Bifrost，配置真实模型 provider，通过 `/agent/chat` 发送“启动 codex cli，新建一个任务/给 Codex 派发任务”类提示，作为 delegated agent-style task 回归样例；断言 session JSONL 中出现 `exec_command` + `write_stdin` 且不出现 `shell_pty` 或 blocking `shell` 执行持续会话命令；该 harness 仅用于本次验证，不落库。
  - 本轮一次性临时 harness：执行 remote executor stdin stream 回归和 CLI interactive 参数解析回归。该 harness 仅用于本次验证，不落库。
  - 本轮一次性临时 harness：在真实 relay/target/caller 链路中覆盖 `remote exec --interactive` 的本地 PTY raw mode、caller-to-client `call_frame` stdin 转发、远端 shell 读入和 Recent Calls 落库。该 harness 仅用于本次验证，不落库。

### 真实场景测试

- 新增 `human_tests/agent-builtin-tools-completeness.md`
- 更新 `human_tests/readme.md`
- 按用例真实执行：
  - `exec_command` 短命令
  - `exec_command` 长命令 + `write_stdin` 轮询
  - `exec_command tty=true` 真实 PTY，`isatty(stdin/stdout)=True True`
  - 本机 Codex CLI interactive session 在真实 PTY 中启动并可被 Ctrl-C 清理
  - 真实 Bifrost `/agent/chat` 经真实模型自行调度 PTY 工具，并向 Codex CLI interactive session 追加引导问题
  - 真实 Bifrost `/agent/chat` 对“启动 Codex CLI/派发 Codex 任务”类自然语言请求会选择 `exec_command` + `write_stdin`，不会再用 `shell_pty` 或 blocking `shell` 执行 `codex exec`/`codex review`
  - 真实 Remote Invoke relay/target/caller 链路中，`remote exec --interactive` 可把本地 PTY 输入转发给远端 shell 程序
  - `view_image` 本地图片 data URL
  - `request_user_input` 当前不可交互错误
  - `tool_search` 默认隐藏、MCP deferred 时暴露并加载搜索结果

## 校验要求

- `cargo test -p bifrost-agent exec_command`
- `cargo test -p bifrost-agent test_exec_command_tty_reports_isatty_true`
- `cargo test -p bifrost-agent terminal_tool_selection -- --nocapture`
- `cargo test -p bifrost-agent model_visible_tool_definitions_prefer_unified_exec_tools -- --nocapture`
- `cargo test -p bifrost-agent --test p1_tools_e2e codex_cli_interactive_starts_in_real_pty -- --ignored --nocapture`
- `cargo test -p bifrost-agent view_image`
- `cargo test -p bifrost-agent tool_search`
- `cargo test -p bifrost-agent --test p1_tools_e2e`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream -- --nocapture`
- `cargo test -p bifrost-cli remote:: -- --nocapture`
- 临时 harness：本地 agent PTY + Codex interactive 回归，不落库。
- 临时 harness：真实 Bifrost `/agent/chat` + 真实模型调度 PTY/Codex interactive 回归，不落库。
- 临时 harness：Remote Invoke stdin forwarding 与 `remote exec --interactive` 真实 relay 链路回归，不落库。
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-06 covers the workspace all-features compile gate for agent message serialization types.
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-07 covers a real Bifrost server on a non-production port and a live `/api/im-gateway/agent/chat` turn that asks the model to use the new tools.
- `human_tests/agent-builtin-tools-completeness.md` TC-ABT-08 covers the final local CI static gate, including IM gateway compile regressions surfaced while validating the tool work.
- 最后按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`
