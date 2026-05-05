# Agent Builtin Tools Completeness 真实场景测试用例

## 功能模块说明

覆盖 Bifrost agent 对齐 Codex 编程内置工具的第一批能力：

1. `exec_command` 短命令一次性完成
2. `exec_command` 长任务返回 session，并由 `write_stdin` 继续交互
3. `view_image` 读取本地图片并返回 data URL
4. `request_user_input` 参数校验与当前不可交互边界
5. `tool_search` 搜索本地已注册核心工具
6. `ChatMessage` 手写序列化类型在 workspace all-features 下可编译
7. 启动真实 Bifrost 服务，通过 `/api/im-gateway/agent/chat` 触发真实模型对话和工具调用
8. 本地 CI 静态门禁覆盖 IM gateway 编译回归

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

### TC-ABT-05: `tool_search` 可发现新增核心工具

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-builtin-tools-target cargo test -p bifrost-agent tool_search -- --nocapture
  ```
- **预期结果**: 测试通过；查询 `exec` 或 `exec command` 能返回 `exec_command`。
- **本次执行结果**: 2026-05-06 执行通过；`tool_search_finds_registered_tool`、`tool_search_lists_core_tools` 测试均返回 ok。

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
    -d '{"session_key":"agent-tools-live-e2e","work_dir":"/Users/eden/work/github/bifrost","message":"请只使用工具完成验证：1) 用 tool_search 搜索 exec_command；2) 用 exec_command 执行 printf BIFROST_TOOL_OK；3) 用 exec_command 创建一个小 PNG 文件到 ./.bifrost-test/agent-live.png；4) 用 view_image 读取这个 PNG；最后回答包含 BIFROST_TOOL_OK 和 image ok。不要使用 mock 数据。"}'
  curl -sS http://127.0.0.1:18880/_bifrost/api/im-gateway/agent/sessions/agent-tools-live-e2e
  ```
- **预期结果**: 服务由当前源码构建启动；工具列表包含 `exec_command`、`write_stdin`、`view_image`、`tool_search`；chat 返回 `success: true`，`tool_calls` 中包含 `tool_search`、`exec_command`、`view_image`，且最终响应或会话详情包含 `BIFROST_TOOL_OK`。
- **本次执行结果**: 2026-05-06 执行通过；当前源码构建的 Bifrost 在 18880 端口启动成功，工具列表包含 `exec_command`、`write_stdin`、`view_image`、`tool_search`；真实 chat 返回 `success: true`，`tool_calls` 记录包含 `set_title`、`tool_search`、两次 `exec_command`、`view_image`，最终响应包含 `BIFROST_TOOL_OK` 和 `image ok`，会话详情记录 12 条消息和 5 条 tool 结果。

### TC-ABT-08: 本地 CI 静态门禁通过 IM gateway 编译回归

- **操作步骤**:
  ```bash
  bash scripts/ci/local-ci.sh --skip-e2e
  ```
- **预期结果**: `cargo fmt (workspace)`、`cargo fmt (desktop)`、`cargo clippy`、`cargo test (workspace)` 全部通过；`crates/bifrost-admin/src/handlers/im_gateway.rs` 不再出现 `String`/`&str` 类型不匹配，`crates/bifrost-admin/src/im_gateway/feishu.rs` 不再出现 `truncate_for_log` 未定义。
- **本次执行结果**: 2026-05-06 执行通过；`bash scripts/ci/local-ci.sh --skip-e2e` 最终报告为 4 passed、0 failed、5 skipped，覆盖 workspace fmt、desktop fmt、clippy、workspace all-features test。

## 清理步骤

```bash
rm -rf ./.bifrost-test/agent-builtin-tools-target
rm -rf ./.bifrost-test/agent-builtin-tools-live
```
