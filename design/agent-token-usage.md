# Agent Token Usage 统计口径

## 功能模块说明

本模块修复内置 Bifrost Agent 对模型 token usage 的统计口径，避免把单次响应的 `total_tokens` 同时当作累计消耗和当前 context 快照使用。

Agent 需要区分两类 token：

- `total_tokens_used`：历史所有模型调用的累计 API 消耗，用于成本、Goal accounting 和总量展示。
- `last_response_tokens`：最近一次模型请求返回的当前 context 快照，用于 `effective_token_count()`、自动压缩判断、`/status` 和 IM 卡片 Context 展示。

## 用户目标验证清单

### 必须实现

- Chat Completions 的 `usage.prompt_tokens` 作为当前 context 快照，`usage.total_tokens` 只进入累计消耗。
- Responses API 的 `usage.input_tokens` 与 OpenAI 兼容 `usage.prompt_tokens` 都能作为当前 context 快照。
- JSONL 持久化同时记录 `tokens` 和 `context_tokens`，恢复时优先用 `context_tokens`，兼容旧事件回退 `tokens`。
- `effective_token_count()` 继续使用最近 context 快照加后续追加消息估算，而不是使用累计 token。

### 必须不破坏

- `total_tokens_used` 仍累加每次模型调用的 `total_tokens`。
- compaction 的 `post_tokens` 仍优先作为压缩后的 context 快照。
- 旧 JSONL 中只有 `tokens` 的 assistant message 仍可恢复。
- Goal token accounting 继续基于累计 `total_tokens_used`。

### 必须真实验证

- 单元测试覆盖 `TokenUsage::context_tokens()`、`track_token_usage(context,total)`、JSONL `context_tokens` 恢复与旧事件兼容。
- E2E/API 真实链路覆盖 mock model usage 中 `prompt_tokens != total_tokens` 时，状态面板 context 使用 prompt/input tokens，累计 tokens 使用 total tokens。
- human_tests 覆盖文档、代码和单元命令静态/真实执行。

## 实现逻辑

1. `TokenUsage` 保留 `prompt_tokens`、`completion_tokens`、`total_tokens`，新增 `context_tokens()`：优先返回 `prompt_tokens`，缺失时回退 `total_tokens`。
2. Chat Completions parser 继续读取 `prompt_tokens`；Responses parser 继续从 `input_tokens` / `prompt_tokens` 读取到同一个字段。
3. turn loop 收到模型响应后调用 `session.track_token_usage(usage.context_tokens(), usage.total_tokens)`：
   - `last_response_tokens = context_tokens`
   - `total_tokens_used += total_tokens`
4. assistant message JSONL 写入：
   - `tokens`：累计用量增量，即响应 `total_tokens`
   - `context_tokens`：当前 context 快照，即响应 `prompt_tokens` / `input_tokens`
5. `load_session_runtime_state()` 回放 assistant message 时优先使用 `context_tokens` 恢复 `last_response_tokens`，没有该字段时回退旧 `tokens`。
6. `scan_session_summary()` 继续只累计 `tokens`，不重复累计 `context_tokens`。

## 依赖项

- `crates/agent/src/types.rs`
- `crates/agent/src/client.rs`
- `crates/agent/src/responses.rs`
- `crates/agent/src/session.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/persistence.rs`
- `crates/agent/src/session/tests.rs`
- `human_tests/agent-token-usage.md`

## 测试方案

### 单元测试

- `token_usage_context_prefers_prompt_tokens`：`prompt_tokens=90,total_tokens=100` 时 context 为 `90`。
- `token_usage_context_falls_back_to_total_tokens`：旧/异常 provider 未返回 input tokens 时回退 `total_tokens`。
- `test_track_token_usage`：context 快照和累计消耗分别写入不同字段。
- `test_record_assistant_message_with_tokens_updates_runtime_summary`：JSONL 记录 `tokens=42,context_tokens=35` 后累计为 `42`、context 快照为 `35`。
- `test_load_session_runtime_state_keeps_context_snapshot_separate_from_cumulative_tokens`：恢复时 `session_end.total_tokens=50000` 不污染最近 context 快照。
- `test_load_session_runtime_state_falls_back_to_total_tokens_for_old_events`：旧 JSONL 兼容。

### E2E 测试

- 更新或复用内置 Agent mock model API 脚本：mock response 返回 `prompt_tokens != total_tokens`，断言 `/status` / `active_status.last_response_tokens` / `estimated_context_tokens` 使用 input tokens，最终 session summary `tokens` 使用 total tokens。
- Responses API 路径使用 `input_tokens` 字段作为同等验收入口。

### 真实场景测试

- `human_tests/agent-token-usage.md` 覆盖设计、代码、单元命令和 E2E 验收入口。
- 真实服务启动必须使用临时 `BIFROST_DATA_DIR` 和 `--no-system-proxy`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `TokenUsage` 字段解析、turn loop 写入、持久化读取与 summary 扫描。
- 执行 `git status --short`、`git diff`。
- 运行 token usage 相关单元测试，若发现累计/快照混用立即修复。

### 第 2 轮

- 复查第 1 轮修复后的 diff，重点检查 Responses API、Chat Completions 和旧 JSONL 兼容。
- 复跑 targeted 单元、human_tests 静态验收和相关 E2E。
- 若第 2 轮仍发现统计口径不一致，继续追加轮次。

## 校验要求

- `cargo test -p bifrost-agent --all-features token_usage context_snapshot runtime_state`
- `cargo test -p bifrost-agent --all-features plan_update_empty`
- 对应 human_tests 逐条执行
- 相关 E2E/API 脚本
- `cargo test --workspace --all-features`
- rust-project-validate

## 文档更新要求

- 新增 `human_tests/agent-token-usage.md` 并更新 `human_tests/readme.md`。
- 如 CLI/API 输出字段名变更，需要同步 README；本轮不改外部字段名，只修正内部口径。

## 残余风险

- 部分 provider 可能只返回 `total_tokens`，此时 context 快照仍只能回退 total tokens，避免丢失状态但无法获得更精确输入 token。
- 如果 provider 返回的 `prompt_tokens` 语义不是当前完整上下文，仍以 provider usage 为准；需要后续 provider-specific 适配。
