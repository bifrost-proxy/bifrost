# Agent Builtin Tools Completeness 真实场景测试用例

## 功能模块说明

覆盖 Bifrost agent 编程内置工具的第一批能力：

1. `exec_command` 短命令一次性完成
2. `exec_command` 长任务返回 session，并由 `write_stdin` 继续交互
3. `view_image` 读取本地图片并返回 data URL
4. `request_user_input` 参数校验与当前不可交互边界
5. `tool_search` 只在 deferred tools 存在时暴露，并返回可加载工具定义
6. `ChatMessage` 手写序列化类型在 workspace all-features 下可编译
7. 启动真实 Bifrost 服务，通过 `/api/im-gateway/agent/chat` 触发真实模型对话和默认直暴工具调用
8. 本地 CI 静态门禁覆盖 IM gateway 编译回归
9. MCP 工具数量达到阈值 `>= 100` 时进入 deferred loading
10. 真实 Bifrost + 大量 MCP tools 验证 `tool_search` 搜索、加载、调用策略生效
11. `exec_command tty=true` 使用真实 PTY，并可启动 Codex CLI interactive session
12. 真实 Bifrost `/agent/chat` 通过真实模型调度 PTY 工具、交互式 Python、Codex CLI interactive session 与追加引导问题
13. 真实 Bifrost `/agent/chat` 对 delegated agent-style/交互/长任务类请求必须使用 `shell_pty`；以“启动 Codex CLI/派发 Codex 任务”为真实回归样例，不能用 blocking `shell` 执行持续会话命令
14. 真实 Bifrost `/agent/chat` 派发 Codex CLI 创建宣传网页，并通过 `write_stdin` 持续观察/追加引导消息

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
- **预期结果**: 测试通过；短命令返回完成 JSON；交互命令返回 `session_id`，`write_stdin` 使用 `chars` 字段写入后可读回 `hello exec`。
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

### TC-ABT-11: `exec_command tty=true` 使用真实 PTY 并驱动 Codex interactive

- **操作步骤**:
  ```bash
	  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent test_exec_command_tty_reports_isatty_true -- --nocapture
	  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent --test p1_tools_e2e exec_command_tool_works_end_to_end -- --nocapture
	  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent --test p1_tools_e2e codex_cli_interactive_starts_in_real_pty -- --ignored --nocapture
	  ```
- **预期结果**: `tty=true` 场景下 `python3 -c 'import os,sys; print(os.isatty(0), os.isatty(1))'` 输出 `True True`；`exec_command_tool_works_end_to_end` 返回持续会话并通过 `write_stdin` 读回输入；Codex CLI 测试不是 `codex --help`，而是在真实 PTY 中启动 interactive session，观察到 Codex 相关界面/登录/欢迎输出后通过 Ctrl-C 清理。
- **本次执行结果**: 2026-05-09 执行通过；`test_exec_command_tty_reports_isatty_true` 输出包含 `True True`；`exec_command_tool_works_end_to_end` 在交互命令中同样确认 stdin/stdout 为 TTY 并通过 `write_stdin` 回显输入；本机 `/opt/homebrew/bin/codex` 版本 `codex-cli 0.128.0` 的 ignored 回归 `codex_cli_interactive_starts_in_real_pty` 已真实执行并通过，证明启动的是 Codex interactive session 而不是 help 文本。

### TC-ABT-12: 真实 `/agent/chat` 调度 PTY、交互式输入和 Codex CLI 追加引导问题

- **操作步骤**:
  ```bash
  # 本轮使用 /tmp 下的一次性临时 harness 执行，测试脚本不落库。
  # harness 必须启动当前源码构建的 Bifrost，使用 --no-system-proxy 和非 9900 端口。
  # harness 必须要求 MODELHUB_AK 存在并配置真实 aidp_crawl provider，禁止 mock provider。
  # harness 调用 POST /_bifrost/api/im-gateway/agent/chat，并要求真实模型依次调用：
  # 1) exec_command tty=true 启动交互式 Python，输出 AGENT_REAL_PTY_READY True True
  # 2) write_stdin 写入 AGENT_REAL_GUIDE_OK
  # 3) exec_command tty=true 启动 codex --sandbox read-only .
  # 4) write_stdin 向 Codex interactive session 追加“请只回答 BIFROST_CODEX_GUIDE_REAL_OK”
  # 5) write_stdin 发送 Ctrl-C 清理 Codex session
  ```
- **预期结果**: `/agent/chat` 返回 `success: true`；响应或 session 详情包含 `AGENT_REAL_PTY_READY`、`True True`、`AGENT_REAL_GUIDE_OK`、`codex --sandbox read-only`、`BIFROST_CODEX_GUIDE_REAL_OK`、`AGENT_REAL_PTY_CHAT_OK`、`BIFROST_CODEX_GUIDE_SENT`；`tool_calls` 包含 `exec_command`、`write_stdin` 和 `tty=true`，证明是 agent 对话真实调度 PTY 工具，而不是 mock 或手动命令。
- **本次执行结果**: 2026-05-09 执行通过；使用 `/tmp` 下的一次性临时 harness（执行后删除，未落库）启动当前源码构建的 Bifrost，真实端口 `65489`，隔离数据目录 `/var/folders/0q/zf2m3_nx6f9gqfd_jx0fcljr0000gn/T//bifrost-agent-real-pty-chat-v0wAZQ`，启动参数包含 `--unsafe-ssl --no-system-proxy`。harness 要求 `MODELHUB_AK` 存在并配置真实 `aidp_crawl` provider，未使用 mock provider。真实 `/agent/chat` 返回 `success: true`，`tool_calls` 包含 `exec_command`、`write_stdin` 和 `tty=true`；session/响应中包含 `AGENT_REAL_PTY_READY True True`、`AGENT_REAL_GUIDE_OK`、`codex --sandbox read-only .`、`BIFROST_CODEX_GUIDE_REAL_OK`、`AGENT_REAL_PTY_CHAT_OK`、`BIFROST_CODEX_GUIDE_SENT`，证明 agent 对话经真实模型调度 PTY 工具，启动 Codex CLI interactive session，并向该 session 追加了引导问题。

### TC-ABT-13: delegated/交互/长任务请求必须使用 `shell_pty`

- **操作步骤**:
  ```bash
  # 本轮使用 /tmp 下的一次性临时 harness 执行，测试脚本不落库。
  # harness 必须启动当前源码构建的 Bifrost，使用 --no-system-proxy 和非 9900 端口。
  # harness 必须要求 MODELHUB_AK 存在并配置真实 aidp_crawl provider，禁止 mock provider。
  # harness 调用 POST /_bifrost/api/im-gateway/agent/chat，使用 Codex CLI 派发作为 delegated agent-style task 回归样例，用户消息使用：
  #   启动codex cli，新建一个任务，检查当前分支的改动内容，做代码review，给出review报告。
  #   注意：必须给 Codex 派发任务，不要自己做 review；如果需要执行 codex exec/review，也要作为可继续观察的交互会话启动。
  # harness 检查 session JSONL 中的 tool_call：
  #   1) 至少出现一次 tool_name = shell_pty，且 command 是持续可观察的前台任务。
  #   2) 不允许出现 tool_name = shell 且 command 是长任务/交互/等待 stdin/delegated agent-style task。
  #   3) 如果模型先错误调用 shell，shell tool 必须返回包含 shell_pty/wait_for_completion=false/write_stdin 的失败指引，后续必须改用 shell_pty。
  ```
- **预期结果**: `/agent/chat` 返回 `success: true`；session 记录证明真实模型请求经过当前源码构建的 Bifrost；delegated/交互/长任务命令通过 `shell_pty` 建立持续会话并返回 `session_id`；没有 blocking `shell` 执行持续会话命令的成功工具调用。
- **本次执行结果**: 2026-05-10 执行通过；使用一次性临时 harness（未落库）启动当前源码构建的 Bifrost，端口 `64283`，隔离数据目录 `/tmp/bifrost-agent-codex-dispatch-t5FSsD`，启动参数包含 `--unsafe-ssl --no-system-proxy`。`GET /_bifrost/api/im-gateway/agent/tools` 返回的模型可见工具顺序前四项为 `shell_pty`、`write_stdin`、`exec_command`、`apply_patch`，legacy `shell` 不再排在会话型终端工具前面。真实 `/agent/chat` 使用默认 `aidp_crawl` provider 和真实模型，请求“启动 codex cli，派发一个极小任务”；返回 `success: true`，`tool_calls` 包含 `shell_pty` 启动 `codex exec '只回答 BIFROST_DISPATCH_PTY_OK'`，参数包含 `wait_for_completion=false`，结果返回 `session_id: 67dd40e9-73da-4926-8e6b-f2c260729107` 和 `exit_indicator: running`；随后调用 `write_stdin` 空输入轮询输出。session JSONL `/tmp/bifrost-agent-codex-dispatch-t5FSsD/agent/sessions/2026/05/09/session-codex-dispatch-pty-regression-1778344744.jsonl` 中没有出现 `tool_name = shell` 执行 `codex exec`/`codex review`。清理回合仅调用 `write_stdin` 向该 `session_id` 发送 Ctrl-C，没有启动新命令。

### TC-ABT-14: 真实 `/agent/chat` 通过 `shell_pty` 启动 Codex CLI 创建宣传网页并追加引导消息

- **操作步骤**:
  ```bash
  # 本轮使用 /tmp 下的一次性临时 harness 执行，测试脚本不落库。
  # harness 必须启动当前源码构建的 Bifrost，使用 --no-system-proxy 和非 9900 端口。
  # harness 必须要求 MODELHUB_AK 存在并配置真实 aidp_crawl provider，禁止 mock provider。
  # harness 调用 POST /_bifrost/api/im-gateway/agent/chat，用户消息要求：
  #   派发给 Codex CLI 创建一个宣传网页 index.html；
  #   启动 Codex 后继续用 write_stdin 观察输出；
  #   追加引导消息，要求页面包含 BIFROST_PROMO_GUIDE_MARKER。
  # harness 检查生成的 index.html 和 session JSONL：
  #   1) index.html 同时包含 BIFROST_PROMO_PAGE_BASE_MARKER 与 BIFROST_PROMO_GUIDE_MARKER。
  #   2) session JSONL 中出现 shell_pty、write_stdin 和 codex。
  #   3) 不依赖 mock provider 或预写文件。
  ```
- **预期结果**: `/agent/chat` 返回 `success: true`；真实模型把 Codex CLI 作为可持续观察任务通过 `shell_pty` 启动，后续使用 `write_stdin` 轮询输出/追加引导；Codex 在隔离 work_dir 中生成 `index.html`，且页面包含基础标记和追加引导标记。
- **本次执行结果**: 2026-05-10 执行通过；使用一次性临时 harness（执行后删除，未落库）启动当前源码构建的 Bifrost，端口 `61674`，启动参数包含 `--unsafe-ssl --no-system-proxy`，真实 `aidp_crawl` provider 可用且未使用 mock。真实 `/agent/chat` 派发 Codex CLI 创建宣传网页，最终生成 `/var/folders/0q/zf2m3_nx6f9gqfd_jx0fcljr0000gn/T/bifrost-agent-codex-promo-wTQbxo/promo-work/index.html`，文件大小 `13450` bytes，包含 `BIFROST_PROMO_PAGE_BASE_MARKER` 与 `BIFROST_PROMO_GUIDE_MARKER`。session JSONL 统计显示 `shell_pty_count=13`、`write_stdin_count=48`，并包含 `codex` 命令记录，证明 agent 通过真实 PTY 会话持续观察 Codex 输出并追加引导消息。

## 清理步骤

```bash
rm -rf ./.bifrost-test/agent-builtin-tools-target
rm -rf ./.bifrost-test/agent-builtin-tools-live
rm -rf ./.bifrost-test/agent-exec-command-tty-target
rm -rf /tmp/bifrost-agent-real-pty-chat-*
rm -rf /tmp/bifrost-agent-codex-dispatch-*
rm -rf /tmp/bifrost-agent-codex-promo-*
```
