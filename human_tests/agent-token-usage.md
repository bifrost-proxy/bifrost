# Agent Token Usage 测试用例

## 功能模块说明

本模块验证内置 Bifrost Agent 的 token usage 统计口径：累计 API 消耗使用 `total_tokens`，当前 Context 快照使用 Chat Completions `prompt_tokens` 或 Responses API `input_tokens`。

## 前置条件

1. 当前目录位于仓库根目录。
2. 已安装 Rust toolchain。
3. 如执行真实服务 E2E，必须使用临时 `BIFROST_DATA_DIR` 且启动参数包含 `--no-system-proxy`。

## 测试用例列表

### TC-ATU-01：设计方案区分累计 token 与 context token

**操作步骤**：
1. 执行：
   ```bash
   test -f design/agent-token-usage.md
   rg -n 'total_tokens_used|last_response_tokens|context_tokens|prompt_tokens|input_tokens' design/agent-token-usage.md
   ```

**预期结果**：
- `design/agent-token-usage.md` 存在。
- 文档明确累计 API 消耗和当前 context 快照是不同字段。
- 文档明确 Chat Completions 使用 `prompt_tokens`、Responses API 使用 `input_tokens`。

### TC-ATU-02：代码写入区分 total tokens 与 context tokens

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'pub fn context_tokens|prompt_tokens > 0|self.total_tokens' crates/agent/src/types.rs
   rg -n 'track_token_usage\(usage\.context_tokens\(\), usage\.total_tokens\)' crates/agent/src/session/turn_loop.rs
   rg -n 'record_assistant_message_with_token_usage|"context_tokens"|or_else\(\|\| event\.content\.get\("tokens"\)\)' crates/agent/src/persistence.rs
   rg -n 'input_tokens|prompt_tokens' crates/agent/src/responses.rs crates/agent/src/client.rs
   ```

**预期结果**：
- `TokenUsage::context_tokens()` 优先使用 `prompt_tokens`，缺失时回退 `total_tokens`。
- turn loop 调用 `track_token_usage(context,total)`，没有把 `total_tokens` 直接作为 context 快照。
- JSONL assistant message 写入 `context_tokens`，恢复时优先读取该字段并兼容旧 `tokens`。
- Chat Completions 和 Responses API parser 都读取了输入 token 字段。

### TC-ATU-03：单元测试覆盖 token usage 口径

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-agent --all-features token_usage -- --nocapture
   cargo test -p bifrost-agent --all-features context_snapshot -- --nocapture
   cargo test -p bifrost-agent --all-features runtime_state -- --nocapture
   ```

**预期结果**：
- `TokenUsage::context_tokens()` 正常路径和 fallback 测试通过。
- `track_token_usage` 测试证明 `total_tokens_used` 使用 total，`last_response_tokens` 使用 context。
- runtime state 恢复测试证明 `context_tokens` 优先且旧事件兼容。

### TC-ATU-04：真实服务 E2E 覆盖 prompt tokens 不等于 total tokens

**操作步骤**：
1. 执行覆盖内置 Agent usage 的 E2E 脚本：
   ```bash
   bash e2e-tests/tests/test_agent_builtin_status_runtime.sh
   ```
2. 检查脚本输出和断言说明，确认 mock model usage 中 context/status 字段来自输入 token 快照，累计消耗来自 total token。

**预期结果**：
- 脚本输出 `PASS`。
- 真实 Bifrost 服务由当前源码启动，并带临时数据目录和 `--no-system-proxy`。
- `/status` 或 active status 中 `last_response_tokens` / `estimated_context_tokens` 没有被累计 `total_tokens_used` 污染。

## 清理步骤

1. 删除测试过程中创建的临时目录。
2. 确认没有残留的 mock model server 或 `bifrost` 进程。
