# Agent Token Usage 测试用例

## 功能模块说明

本模块验证内置 Bifrost Agent 的 token usage 统计口径：累计 API 消耗使用 `total_tokens`，当前 Context 快照使用 Chat Completions `prompt_tokens` 或 Responses API `input_tokens`。同时验证 AI Chat 输入框上方的 token HUD 能低调展示实时消耗、context 占用比例和压缩状态。

## 前置条件

1. 当前目录位于仓库根目录。
2. 已安装 Rust toolchain。
3. 已安装 Web 依赖（`web/node_modules`）。
4. 如执行真实服务 E2E，必须使用临时 `BIFROST_DATA_DIR` 且启动参数包含 `--no-system-proxy`。

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

### TC-ATU-05：AI Chat 输入框上方展示 token HUD

**操作步骤**：
1. 执行前端 targeted E2E：
   ```bash
   cd web && pnpm exec playwright test tests/ui/agent-chat.spec.ts -g "token HUD"
   ```
2. 检查 Playwright 输出，确认用例 `AI Agent Chat token HUD stays subtle above the composer` 通过。

**预期结果**：
- 测试通过 Playwright mock route 注入 Agent Chat SSE telemetry；后端真实服务 token 口径由 TC-ATU-04 覆盖。
- AI Chat 输入框上方显示 `Tokens 2,468`、`Context 35%`、`Compression 1 · Started`。
- HUD 位于 composer 输入框上方，高度不超过 36px，不遮挡输入框。
- 切换到暗色主题后，HUD 仍可读且数值保持可见。

### TC-ATU-06：AI Chat token HUD 截图留证

**操作步骤**：
1. 执行截图 E2E：
   ```bash
   cd web && pnpm exec playwright test tests/ui/agent-chat.spec.ts -g "token HUD" --update-snapshots
   ```
2. 或在常规 targeted E2E 通过后，从 `test-results/` 保存 trace/截图路径作为交付证据。

**预期结果**：
- 交付摘要包含截图或 trace 路径。
- 截图中 HUD 位于输入框上方，显示 token、context、compression 三项，视觉层级低于主输入区。

## 清理步骤

1. 删除测试过程中创建的临时目录。
2. 确认没有残留的 mock model server 或 `bifrost` 进程。
3. 删除不需要保留的 Web test 临时 trace、video、screenshot 文件；需要作为交付证据的截图路径除外。
