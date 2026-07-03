# Agent Builtin Tools Completeness

## 背景

Bifrost 内置的 coding agent 面向模型的“工具面”过去在多轮真实使用中暴露了一批可靠性和一致性问题：

- 终端类工具历史上并存 `shell`、`shell_pty`、`shell_command`、`local_shell` 与 `exec_command` 五种形态，模型可见 schema 差异大；碰到“启动 Codex CLI 派发任务”这类需求时，模型经常选到 blocking `shell` 执行 `codex exec review`，导致没有 `session_id`、没法用 `write_stdin` 观察/输入/取消。
- `write_stdin` 存在两套协议：数字 `session_id` + `chars` 与旧字符串 `session id` + 隐藏 `input` 字段，模型经常混用。
- MCP 工具在项目量 >= 100 时会撑爆模型可见 tools 列表；`tool_search` 作为 deferred discovery 入口不能默认常驻。
- `view_image`、`request_user_input`、`request_permissions` 等标准工具语义在 Bifrost 内没有完整覆盖，模型看到空实现或错误说明。
- Remote Invoke 场景（在远端机器上跑 coding-agent）缺 stdin forwarding，没法在 `bifrost remote exec` 交互式命令中传入按键。

本方案统一收敛 Bifrost agent 的工具面，把模型可见入口收敛为最小可靠集，把 deferred discovery、真实 PTY、Remote stdin forwarding 都补齐。

## 参照工具面

编程标准工具集合（Codex 等价面）：

- 环境执行：`exec_command` + `write_stdin`（模型可见）；旧 `shell`/`shell_pty`/`shell_command`/`local_shell` 全部下线。
- 文件修改：`apply_patch`
- 进度与目标：`update_plan`、`get_goal`、`create_goal`、`update_goal`
- 本地视觉输入：`view_image`
- 用户交互与权限：`request_user_input`、`request_permissions`
- 工具发现：`tool_search`、`tool_suggest`/`request_plugin_install`
- MCP resource：`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource`
- 子 Agent 协作：`spawn_agent`、`wait_agent`、`close_agent`、`send_input`/`resume_agent` 或 v2 的 `send_message`/`followup_task`/`list_agents`
- 可选扩展：`web_search`、`image_generation`、`code_mode`、`spawn_agents_on_csv`、`report_agent_job_result`

### `tool_search` 对齐结论

`tool_search` 不是普通常驻本地工具，而是 deferred tool discovery 的模型可见入口。Bifrost 按以下方式对齐：

| 行为要求 | Bifrost 对齐方式 |
| --- | --- |
| `DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD` 固定为 `100` | `crates/agent/src/mcp/mod.rs` 使用同名阈值 `100` |
| 仅在启用强制 defer 或 deferred MCP 数量 `>= 100` 时进入 deferred loading | Bifrost 当前无 feature flag/config gate，默认按数量阈值 |
| 不 defer 时候选 MCP 全部直暴，`deferred_tools=None` | Bifrost 低于 100 个 MCP 工具时全部直接加入 `tools`，不暴露 `tool_search` |
| defer 时只保留显式启用 App 工具直暴，其余 deferred；空 deferred 不返回 | Bifrost 暂无 App connector，defer 时 MCP 全部放入 deferred；为空则不暴露 `tool_search` |
| dynamic tools 仅 `defer_loading=true` 且 namespace 满足时进入 deferred | Bifrost 无 dynamic tools 与 Responses API namespace，第一阶段只实现 MCP deferred `ToolDefinition` |
| 只有存在 deferred MCP 或 dynamic tools 时才注册 `tool_search` | `ToolRegistry::with_defaults` 不注册 `tool_search`；`AgentSession` 仅在 `tool_search_entries` 非空时加入 |
| deferred specs handler 仍注册，初始不可见 | MCP manager 始终保留全部 MCP 工具定义与调用路由；搜索命中后同一 turn 后续模型调用可命中 |
| schema `{query, limit}`，`query` 必填，`additionalProperties=false` | Bifrost `tool_search` schema 完全对齐 |
| 搜索结果输出可注入下一次工具列表的 loadable specs | Bifrost 使用 Chat Completions `ToolDefinition`，在 Agent loop 追加到下一次请求 `tools` |
| handler 基于 entries `search_text` 构建 BM25 index | `ToolSearchTool` 用 `bm25` crate 建索引 |
| 空 query / `limit=0` 返回可见错误；默认 limit 为 8 | `TOOL_SEARCH_DEFAULT_LIMIT = 8`；空 query 与 `limit=0` 明确报错 |
| deferred entries 空时返回空 `tools` | 运行时为空时不暴露 `tool_search`；直接构造空 handler 时也返回空结果 |
| BM25 搜索后输出 coalesced loadable specs；按 bucket 限制 | 无 computer-use 特例，无 namespace 合并；BM25 结果直接返回 `ToolDefinition`；为下划线工具名补精确子串兜底 |
| entry 从排序后的 MCP + dynamic tools 构建 | entry 从 MCP exposure deferred tools 构建；dynamic tools 待接入 |
| deferred specs 初始不暴露，被搜索命中后加载到后续 model request | `session.rs` 在 `tool_search` 成功后解析 `{"tools":[...]}` 并追加到同一 turn 后续模型请求 |

## 用户目标验证清单

### 必须实现

- 模型可见终端工具只保留 `exec_command` 与 `write_stdin`，schema、错误消息、session 语义完全一致。
- `exec_command` 支持短命令、长命令 yield、pipe / PTY 两种后端；PTY 模式下 `os.isatty(0)`、`os.isatty(1)` 均为 True。
- `write_stdin` 数字 `session_id` 唯一合法，旧字符串 session id 与隐藏 `input` 字段被拒绝。
- `view_image` 支持本地图片路径 + `detail=original`，验证存在且为常见图片类型，返回 data URL。
- `request_user_input` 参数完整校验；当前无交互 UI 通道时返回明确错误。
- `tool_search` 只在存在 deferred tools 时才暴露；默认 limit=8；空 query/`limit=0` 报错。
- MCP tool 数量 `>= 100` 时进入 deferred loading，`tool_search` 出现。
- Codex CLI interactive session 可在真实 PTY 中启动，模型可通过 `write_stdin` 追加引导问题或 Ctrl-C 清理。
- `bifrost remote exec --interactive` 支持本地 PTY raw mode → caller-to-client `call_frame` → 远端 shell stdin 转发。
- 默认 base instructions 明确“执行 shell/terminal 命令统一使用 `exec_command`，交互程序 `tty=true`，观察/输入通过 `write_stdin`”。

### 必须不破坏

- `apply_patch`、`write_file`、`read_file`、`list_directory`、`switch_workdir`、`enter_worktree`、`exit_worktree`、`update_plan`、`set_title`、`get_goal`/`create_goal`/`update_goal` 语义完整保留。
- MCP resource 工具 `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` 仍作为 direct core tools 暴露，不参与 deferred threshold。
- 已有 MCP server 单元测试全部通过。
- IM Gateway `/api/im-gateway/agent/chat` 编译与消息序列化兼容。
- Remote Invoke 其它 subcommand（file、traffic、grant）不受影响。

### 必须真实验证

- 用真实 Bifrost + 真实模型 provider（不允许 mock）跑 `/agent/chat`，观察模型自行选择 `exec_command` + `write_stdin` 完成 Python 交互与 Codex CLI 派发。
- 本地存在 `codex` 时启动 interactive session，`write_stdin` 追加输入 + Ctrl-C 清理，全流程无 hang。
- Remote Invoke 通过真实 relay/target/caller 三节点链路验证 `bifrost remote exec --interactive` 输入转发落到远端 shell。
- 手动执行 `human_tests/agent-builtin-tools-completeness.md` 所有 TC-ABT-XX 用例。

## 产品语义

### 终端工具形态收敛

对照 `~/work/github/codex` 后，关键经验是启用 unified exec 时模型可见工具收敛为 `exec_command` + `write_stdin`；在 Bifrost 当前无兼容压力的前提下，旧 shell 类 handler 直接删除，可靠性来自单一路径，而不是并行维护 `shell_pty` 与 `exec_command` 两套模型协议。

默认 base instructions 明确：凡是需要执行 shell/terminal 命令，都统一使用 `exec_command`，保存返回的 `session_id`，后续通过 `write_stdin` 观察、追加输入或 Ctrl-C 清理；TUI/readline/交互式终端程序设置 `tty=true`。历史 `shell` 与 `shell_pty` handler 已删除，`write_stdin` 与 `exec_command` 同文件实现并共享同一个 `ExecSessionManager`；模型可见终端入口只保留 `exec_command` / `write_stdin`。

### 长任务与短任务统一协议

- 短命令：`exec_command` 在 `yield_time_ms` 内完成，返回 `exit_code` 且不保留 session。
- 长命令：`yield_time_ms` 内未结束，返回数字 `session_id` 且 `exit_code=null`；后续通过 `write_stdin` 轮询。
- 是否仍在运行由 `session_id` 非空且 `exit_code` 为空表达，不额外定义 `running` 字段。
- 完成判断来自 `try_wait()` / PTY child status，不从 shell prompt、stdout sentinel 或模型推测得出。
- Ctrl-C 通过 `write_stdin` 发送，终止子进程并清理后台 session；工具结果按主动取消成功处理并保留真实 exit code。

### `tool_search` 暴露时机

- 默认不常驻本地工具集。
- MCP 工具数 `>= DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD (100)` 时启用 deferred loading，`tool_search` 才加入 tools。
- dynamic tool 后续接入时按 `defer_loading: true` 直接触发。
- 搜索结果 `{"tools":[...]}` 每项是可加入下一次模型请求的 `ToolDefinition`；Agent loop 在同 turn 后续模型调用前把返回工具定义追加到 `tools`。

## 技术细节

### 关键文件与常量

- `crates/agent/src/tools/mod.rs`：工具注册入口 `ToolRegistry::with_defaults`；`exec_command`、`write_stdin`、`view_image`、`request_user_input`、`apply_patch`、`update_plan`、`set_title` 等挂载。
- `crates/agent/src/tools/exec_command.rs`：
  - `pub const MIN_YIELD_TIME_MS: u64 = 250;`
  - `pub const MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS: u64 = 5_000;`
  - `pub const MAX_YIELD_TIME_MS: u64 = 30_000;`
  - `pub const DEFAULT_EXEC_YIELD_TIME_MS: u64 = 10_000;`
  - `pub const DEFAULT_WRITE_STDIN_YIELD_TIME_MS: u64 = 250;`
  - `pub const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;`
  - `pub const MAX_EXEC_SESSIONS: usize = 64;`
  - `pub struct ExecCommandTool { session_manager: Arc<ExecSessionManager> }`
  - `pub struct WriteStdinTool { session_manager: Arc<ExecSessionManager> }`
  - `use portable_pty::{native_pty_system, CommandBuilder, PtySize};`
- `crates/agent/src/tools/tool_search.rs`：
  - `pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";`
  - `pub const TOOL_SEARCH_DEFAULT_LIMIT: usize = 8;`
  - `pub struct ToolSearchEntry { definition, source, search_text }`
  - `pub struct ToolSearchTool { entries: Vec<ToolSearchEntry> }`
  - `pub fn tool_search_definition(deferred_entries: &[ToolSearchEntry]) -> ToolDefinition`
  - `pub fn parse_loadable_tool_definitions(output: &str) -> Vec<ToolDefinition>`
- `crates/agent/src/mcp/mod.rs`：`DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD = 100`；deferred exposure 逻辑。
- `crates/agent/src/session.rs`：`AgentSession` 组装 model-visible tools；`tool_search` 命中后追加 loaded tool 定义。
- `crates/agent/src/prompts/base_instructions/default.md`：默认 base instructions，统一 terminal tool 说明。
- `crates/agent/tests/p1_tools_e2e.rs`：端到端工具级 P1 回归。
- `crates/bifrost-admin/src/remote_invoke/executor.rs`：Remote Invoke shell exec，stdin stream 转发。
- `crates/bifrost-cli/src/commands/remote.rs`：`remote exec --interactive/--stdin/--pty` CLI 参数与本地 PTY raw mode。

### `exec_command` schema

```json
{
  "name": "exec_command",
  "parameters": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "cmd": { "type": "string" },
      "workdir": { "type": "string" },
      "shell": { "type": "string" },
      "login": { "type": "boolean" },
      "tty": { "type": "boolean" },
      "yield_time_ms": { "type": "integer", "minimum": 250, "maximum": 30000 },
      "max_output_tokens": { "type": "integer" }
    },
    "required": ["cmd"]
  }
}
```

输出：

```json
{
  "chunk_id": 1,
  "wall_time_seconds": 0.42,
  "exit_code": 0,          // 长任务未结束时为 null
  "session_id": 17,        // 长任务；短任务无此字段
  "original_token_count": 512,
  "output": "..."
}
```

### `write_stdin` schema

```json
{
  "name": "write_stdin",
  "parameters": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "session_id": { "type": "integer" },
      "chars": { "type": "string" },
      "yield_time_ms": { "type": "integer", "minimum": 250, "maximum": 30000 },
      "max_output_tokens": { "type": "integer" },
      "since_chunk_id": { "type": "integer" }
    },
    "required": ["session_id"]
  }
}
```

拒绝规则：

- 非数字 `session_id` 报错。
- `input` 字段被移除；旧协议客户端会拿到错误提示。
- 空 `chars` 且未指定 `yield_time_ms` 时按 `MIN_EMPTY_WRITE_STDIN_YIELD_TIME_MS`（5s）等待。

### Remote Invoke interactive

- caller CLI：`bifrost remote exec --interactive --shell-text "..."`。
- envelope 携带 `stdin_mode`、`pty` (含 `rows`, `cols`) 与 `output_mode`。
- caller 开启 raw mode，把本地 stdin 加密为 caller-to-client `call_frame`，通过 relay `/calls/{call_id}/input` 发送。
- interactive 模式默认跳过普通 streaming Done digest 校验（PTY 字节流可能被 legacy exit 收敛，不能把缺失 digest 当命令失败）。
- target worker 在 active call map 登记 stdin channel，收到 frame 后解密并转发到 executor。
- executor 对允许 stdin 的 shell command 打开 child stdin 并持续写入。
- 取消走既有 call cancel 通道；raw mode guard 负责退出时恢复终端。
- 当前先同步启动时终端尺寸；远端运行期 resize frame 与远端真 PTY resize 作为后续增强。

## CLI 交互

- `bifrost remote exec --interactive` 全新交互模式；`--stdin` 简单转发；`--pty` 强制 PTY。
- `bifrost agent chat`（若有）默认使用最新工具面。

## Web UI 交互

- Agent Chat 面板不区分工具形态；模型返回的 `exec_command` / `write_stdin` 输出以终端块渲染。
- `view_image` 返回的 data URL 直接在对话中缩略图展示。
- `request_user_input` 触发时，UI 显示回退错误（第一版无交互 UI 通道时如实展示）。

## Admin API

- 无外部 API 字段变化。工具面通过 model tool schema 暴露；Admin API 只关心 Remote Invoke stdin stream 传输。
- Remote Invoke 相关：`POST /_bifrost/api/remote/calls/{call_id}/input` 传输 caller-to-client `call_frame`（stdin）。

## Sync / 导入导出 / 分享边界

不涉及。

## 实现切分

### 第一批：编程核心工具

1. **`exec_command`**
   - 直接 spawn pipe/PTY 子进程；后台保留 child handle、stdin writer、stdout/stderr capped buffer、exit 状态。
   - 完成判断来自 `try_wait()` / PTY child status。
   - Bifrost 当前不实现 OS sandbox/approval，不暴露 `sandbox_permissions`、`justification`、`prefix_rule`。
2. **`write_stdin`**
   - 兼容标准 schema：`session_id`、`chars`、`yield_time_ms`、`max_output_tokens`；额外暴露 `since_chunk_id` 用于按 chunk 游标拉新增输出。
   - 只查找 `exec_command` 真实进程会话；`shell_pty` session manager 或 fallback 全部删除。
   - 空输入轮询不得先清空旧 buffer，避免长命令在两次 poll 之间完成时丢失输出。
3. **`view_image`**
   - 接受本地图片路径与可选 `detail=original`。
   - 验证路径存在且为文件，读取为 data URL；限制常见图片后缀。
4. **`request_user_input`**
   - 对齐标准 schema 与参数校验。
   - Bifrost 当前没有交互 UI 挂起/恢复通道；以明确错误返回，避免模型误以为已经获得用户输入。
5. **`tool_search` 暴露时机与 deferred loading**
   - 只有存在 deferred tools 时才加入当前 turn 的模型可见工具列表。
   - MCP 工具数量阈值：`>= 100` 启用 deferred loading。
   - `tool_search` 输出 `{"tools":[...]}`，Agent loop 在同 turn 后续模型调用前把返回的工具定义加入 `tools`。
   - 默认 limit=8；空 query 与 `limit=0` 明确错误。

### 第二批：协作与外部能力（后续）

- `spawn_agent`/`wait_agent`/`close_agent`/`send_input`/`resume_agent` 需要独立 AgentSession 管理、后台 turn 调度、消息队列、状态持久化和取消语义。禁止只返回假 agent id。
- `request_permissions` 需要与 Bifrost 本地/远程执行权限模型、Shell Access、File Access 或未来 sandbox 统一。
- `web_search` 与 `image_generation` 需要明确供应商、网络策略、密钥来源与输出事件协议。
- `code_mode` 需要独立 JS/TS 工具运行时和工具结果适配层，属于较大架构工作。

### 第三批：真实 PTY 与远端交互输入

1. **本地 `exec_command tty=true`**：`portable-pty` 创建真实 PTY；默认 `80x24`、`TERM=xterm-256color`；工作目录/shell 参数沿用原 `exec_command`。
2. **Codex CLI 交互回归**：新增真实回归启动本机 Codex CLI interactive 入口，通过 `write_stdin` 发送 Ctrl-C 清理；黑盒真实模型回归启动当前源码构建的 Bifrost，配置真实 `aidp_crawl` provider，经 `/agent/chat` 让模型自行调用 `exec_command tty=true` 启动交互 Python、`write_stdin` 写入引导文本，再启动 `codex --sandbox read-only .` interactive session 并通过 `write_stdin` 追加引导问题；标记为 ignored 以避免 CI 失败。
3. **终端工具形态收敛策略**：如“产品语义”所述；模型可见入口只保留 `exec_command` / `write_stdin`。
4. **Remote Invoke stdin forwarding**：如“技术细节 Remote Invoke interactive”所述。

## 测试方案

### 单元测试

- `exec_command_returns_completed_output`：短命令一次调用返回 `exit_code=0` 和输出。
- `exec_command_yields_session_and_write_stdin_polls_to_exit`：长命令返回数字 `session_id` 且 `exit_code=null`，随后 `write_stdin` 轮询到最终 `exit_code=0` 与末尾输出，session 被清理。
- `exec_command_write_stdin_drives_pipe_process`：pipe 模式启动等待 stdin 的 Python 进程，`write_stdin` 写入后返回真实输出与最终 exit code。
- `exec_command_ctrl_c_terminates_running_process`：长时间运行 pipe 进程收 Ctrl-C 后被终止并清理 session。
- `test_exec_command_tty_reports_isatty_true`：`tty=true` + Python `os.isatty(0/1)` 输出 `True True`。
- `write_stdin_rejects_legacy_protocol_fields`：拒绝旧字符串 session id 与隐藏 `input`。
- `test_default_base_instructions_describe_terminal_tool_selection`：base instructions 包含统一使用 `exec_command`、`tty=true`、`write_stdin` 的规则，不再出现 `shell`/`shell_pty` 推荐。
- `model_visible_tool_definitions_prefer_unified_exec_tools`：模型可见工具含 `exec_command`、`write_stdin`；不含 legacy `shell`/`shell_pty`。
- `terminal_tools_reject_non_schema_legacy_arguments`：`exec_command` 拒绝未暴露的权限/沙箱字段；`write_stdin` 拒绝隐藏 `input` 与旧字符串 session id。
- `view_image_rejects_missing_file`：缺失路径明确错误。
- `view_image_returns_data_url`：PNG/JPEG/GIF/WebP 返回 data URL。
- `request_user_input_validates_questions`：空 options 或超过 3 个问题返回错误。
- `tool_search_is_hidden_without_deferred_tools`（位于 `crates/agent/tests/p1_tools_e2e.rs`）：默认工具不含 `tool_search`。
- `tool_search_returns_loadable_deferred_tool_definitions`：查询 deferred MCP 工具返回可加载 `ToolDefinition`。
- `mcp_tool_exposure_defers_at_codex_threshold`：MCP 工具数 = 100 时触发 deferred loading。

### E2E 测试

- 扩展 `crates/agent/tests/p1_tools_e2e.rs`：
  - `exec_command_tool_works_end_to_end`：短命令一次完成、长命令 yield session + `write_stdin` 轮询到 exit、TTY 交互命令 stdin/stdout 均为 TTY、stdin 写入回显。
  - `codex_cli_interactive_starts_in_real_pty`：本地安装 Codex CLI 时启动真实 interactive session，`write_stdin` 发送 Ctrl-C 清理（`--ignored`）。
  - `view_image_tool_works_end_to_end`
  - `tool_search_is_hidden_without_deferred_tools`
  - `tool_search_returns_deferred_mcp_tools`
- 新增 terminal E2E 临时 harness（不落库）：
  - PTY isatty 单测 + `exec_command + write_stdin` P1 E2E；本地存在 `codex` 时主动运行 Codex interactive ignored 回归。
  - 启动真实 Bifrost + 真实模型 provider，经 `/agent/chat` 让模型实际调用 `exec_command tty=true`、`write_stdin`、Codex CLI interactive session 和追加引导问题；不允许降级 mock provider。
  - `/agent/chat` 发送“启动 codex cli，新建一个任务/给 Codex 派发任务”类提示，作为 delegated agent-style task 回归；断言 session JSONL 中出现 `exec_command` + `write_stdin` 且不出现 `shell_pty` 或 blocking `shell`。
  - Remote executor stdin stream 回归 + CLI interactive 参数解析回归。
  - 真实 relay/target/caller 链路覆盖 `remote exec --interactive` 的本地 PTY raw mode、caller-to-client `call_frame` stdin 转发、远端 shell 读入、Recent Calls 落库。

### 真实场景测试 human_tests

新增/维护 `human_tests/agent-builtin-tools-completeness.md`，用例包括：

- TC-ABT-01：`exec_command` 短命令。
- TC-ABT-02：`exec_command` 长命令 + `write_stdin` 轮询。
- TC-ABT-03：`exec_command tty=true` 真实 PTY，`isatty(stdin/stdout)=True True`。
- TC-ABT-04：本机 Codex CLI interactive session 在真实 PTY 中启动并可被 Ctrl-C 清理。
- TC-ABT-05：真实 Bifrost `/agent/chat` 经真实模型自行调度 PTY 工具，并向 Codex CLI interactive session 追加引导问题。
- TC-ABT-06：覆盖 workspace all-features 编译 gate（agent message serialization 类型）。
- TC-ABT-07：真实 Bifrost 非生产端口 + `/api/im-gateway/agent/chat` live turn 让模型使用新工具。
- TC-ABT-08：最终本地 CI 静态 gate，包括本次工作暴露的 IM gateway 编译回归。
- TC-ABT-09：`view_image` 本地图片 data URL。
- TC-ABT-10：`request_user_input` 当前不可交互错误。
- TC-ABT-11：`tool_search` 默认隐藏、MCP deferred 时暴露并加载搜索结果。
- TC-ABT-12：`bifrost remote exec --interactive` relay/target/caller 链路 stdin 转发。

同步更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

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
- 临时 harness（不落库）：本地 agent PTY + Codex interactive 回归；真实 Bifrost `/agent/chat` + 真实模型调度 PTY/Codex interactive 回归；Remote Invoke stdin forwarding 与 `remote exec --interactive` 真实 relay 链路回归。
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 最后按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：终端工具是否收敛为 `exec_command` + `write_stdin`；`view_image`、`request_user_input`、`tool_search` 是否覆盖到位。
- 复核 diff：`crates/agent/src/tools/`、`crates/agent/src/session.rs`、`crates/agent/src/mcp/mod.rs`、`prompts/base_instructions/default.md`、`crates/bifrost-admin/src/remote_invoke/`、`crates/bifrost-cli/src/commands/remote.rs`、`human_tests/agent-builtin-tools-completeness.md`。
- 重点 review：是否存在残留 `shell`/`shell_pty` handler；`write_stdin` 是否严格拒绝旧协议；`tool_search` 是否只在 deferred 时暴露；Remote interactive raw mode 是否有 leak。
- 复测：agent 单元测试 + P1 E2E + Remote Invoke stdin harness。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- 再次检查 `git status --short`、`git diff`、新增文件与 human_tests 索引。
- 重点 review：真实模型 provider 场景下 `/agent/chat` 是否稳定；base instructions 修改是否与 IM Gateway prompt 相容；Codex CLI interactive 用例是否在缺 `codex` 时优雅跳过。
- 复测：失败路径重跑，必要时补充真实 CLI/Web 操作。

## 风险与决策点

- **删除旧 shell 类工具的兼容性**：Bifrost 内部无外部客户端依赖，直接删除 `shell`/`shell_pty` 是可靠性的最短路径。若未来提供 SDK 版本回退需求，可在 base instructions 中显式导出“旧工具已下线”的错误消息，而不是恢复实现。
- **`request_user_input` 无交互通道**：第一版明确错误，避免模型误以为已获得用户输入。后续接入 IM Gateway/WebUI 交互队列时再返回真实等待语义。
- **`tool_search` 阈值 100**：与 Codex 对齐；若企业内部 MCP 数量普遍 < 100，`tool_search` 永不出现，属于设计意图。若产品要求“任意 MCP 都走搜索”，可加 config flag `mcp_defer=always` 覆盖。
- **Codex CLI 真实回归标记 `--ignored`**：CI 机器可能无 `codex`，标记 ignored 避免误红；本地和 human_tests 主动执行。
- **Remote interactive PTY 尺寸**：第一版只同步启动尺寸；不做 SIGWINCH resize。若用户强需 resize，后续加 caller `RESIZE` frame + target `pty.resize()`。
- **`view_image` 大文件**：以后缀白名单限制常见图片类型，避免任意大文件被读成 data URL 撑爆上下文。若需要 PDF 等，走另一个工具而非扩 `view_image`。
- **`spawn_agent` 系列未实现**：第一版明确不提供假实现。文档要求禁止只返回假 agent id；模型看到工具不存在会走替代路径（`exec_command` 拉子进程或人工介入）。
