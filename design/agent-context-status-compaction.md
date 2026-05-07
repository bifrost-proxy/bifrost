# Agent Context Status 与自动压缩修复

## 功能模块说明

本次修复聚焦 Bifrost Agent turn loop 中上下文 token 状态与自动压缩触发的口径一致性。用户可见症状是运行中 `/status` 或会话状态卡片显示 `Context 用量` 已远超 `model_context_window`，但 `压缩次数` 增长很少，导致用户无法判断真实上下文压力，也可能延迟自动压缩。

## 现状与根因

相关代码路径：

- `crates/agent/src/session_status.rs`
  - `refresh_active_turn_status()` 使用 `session.estimate_tokens()` 更新运行中状态。
  - `format_active_turn_status_text()` 将该估算值渲染为 `Context 用量`。
- `crates/agent/src/session.rs`
  - `AgentSession::track_token_usage()` 同时维护 `total_tokens_used` 与 `last_response_tokens`。
  - `AgentSession::effective_token_count()` 优先返回 `last_response_tokens`，否则返回 `estimate_tokens()`。
- `crates/agent/src/compact.rs`
  - `should_compact()` 用 `effective_token_count()` 判断是否超过 `compact_threshold`。
  - `compact_session()` 的摘要模型调用也调用 `session.track_token_usage()`。

根因是 `last_response_tokens` 的语义混用了两类数据：

1. 普通模型请求的 `usage.total_tokens` 可近似代表“最近一次送入模型的上下文 + 输出”。
2. 压缩摘要模型请求的 `usage.total_tokens` 只代表压缩请求本身，不代表压缩后 `session.history` 的当前上下文大小。

压缩完成后，摘要模型请求的 token 被写入 `last_response_tokens`，后续 `should_compact()` 优先读取它，就可能跳过本应继续执行的压缩。工具结果、排队用户消息等追加进 `session.history` 后，也没有统一使 `last_response_tokens` 失效，旧的 API token 快照会继续遮蔽真实历史增长。

## 实现逻辑

### 1. 拆分 token accounting 语义

在 `AgentSession` 中保留累计 token 统计，但区分“普通模型响应快照”和“后台/压缩模型调用消耗”：

- 普通 turn 模型响应继续更新：
  - `last_response_tokens`
  - `total_tokens_used`
- 压缩摘要模型响应只累加：
  - `total_tokens_used`
  - 不更新 `last_response_tokens`

这样 `实时 token: 累计 ...` 仍包含压缩消耗，但自动压缩判断不会把压缩请求 usage 当作当前上下文快照。

### 2. history 变化时失效旧上下文快照

当追加新用户消息、图片用户消息、assistant tool calls 或 tool result 时，清空 `last_response_tokens`，让下一次 `should_compact()` 回到 `estimate_tokens()`。

最终 assistant 文本消息不强制清空该字段，因为普通模型响应的 `usage.total_tokens` 已包含本次 completion，短时间内仍可作为最近请求快照；下一轮用户输入追加时会失效。

### 3. 自动压缩判断使用不小于估算值的上下文口径

`effective_token_count()` 不再让 `last_response_tokens` 单独遮蔽 `estimate_tokens()`。如果两者都存在，返回二者较大值，避免 stale API 快照低估当前历史。这样当 `Context 用量` 显示超过阈值时，`should_compact()` 的判断不会相反。

### 4. 状态文案保留累计与当前上下文的区别

运行中状态继续同时展示：

- `实时 token: 累计 ...，最近响应 ...`
- `Context 用量: ~estimated / window (...)`

本次不改变前端/接口字段，只修复计算语义。若后续要进一步降低歧义，可以单独把展示文案调整为 `当前上下文估算`。

### 5. 压缩事件统计覆盖所有触发路径

`manual`、`pre_turn`、`mid_turn` 和 context window overflow 后的 emergency compaction 都通过同一个 recorder helper 写入 `compaction` 事件。事件 metadata 包含：

- `trigger`、`reason`、`phase`
- `pre_tokens`、`post_tokens`、`tokens_saved`、`messages_removed`
- `compaction_count`
- `total_tokens`
- emergency 路径额外包含 `emergency: true`

这样内存中的 `compaction_count`、会话文件中的 `compaction` 事件，以及历史摘要扫描使用的累计 token 口径保持一致。

## 依赖项

- `crates/agent/src/session.rs`
- `crates/agent/src/compact.rs`
- `crates/agent/src/session_status.rs`
- `e2e-tests/tests/test_agent_builtin_status_runtime.sh`
- `human_tests/agent-builtin-commands.md`
- `human_tests/readme.md`

## 测试方案

### 单元测试

1. `AgentSession::effective_token_count` 回归：
   - 构造 `last_response_tokens = 100`，再追加大体积 tool result。
   - 断言 `last_response_tokens` 被清空，`effective_token_count()` 回到 history 估算。
2. `compact::should_compact` 回归：
   - 构造 history 估算值超过阈值，同时模拟较小的最近响应 token。
   - 断言仍触发压缩，避免旧 API 快照遮蔽真实上下文。
3. `compact_session` token accounting 回归：
   - 压缩模型调用只增加 `total_tokens_used`，不把摘要请求 usage 留在 `last_response_tokens`。
4. `record_compaction_event` metadata 回归：
   - emergency compaction 事件必须包含 `emergency: true` 与 `total_tokens`。
   - 事件中的 `compaction_count` 必须等于 session 当前累计压缩次数。

### E2E 测试

更新 `e2e-tests/tests/test_agent_builtin_status_runtime.sh`：

1. mock provider 增加一条工具调用链路，返回较小 `usage.total_tokens`。
2. 工具调用产生大体积输出，使 `session.history` 增长。
3. 在运行中 `/status` 验证 `active_status.estimated_context_tokens` 反映大体积工具结果。
4. 验证 `Context 用量` 与 `active_status.estimated_context_tokens` 一致，不被较小的 `last_response_tokens` 遮蔽。

### 真实场景测试（human_tests）

更新 `human_tests/agent-builtin-commands.md`：

1. 新增 `TC-BC-21`：运行中 `/status` 在工具结果追加后展示当前上下文估算，而不是最近模型响应 token。
2. 新增 `TC-BC-22`：自动压缩判断不被压缩摘要模型调用 usage 遮蔽；当 history 估算仍超阈值时，下一次检查仍可继续触发压缩。
3. 新增 `TC-BC-23`：emergency compaction 也必须记录 `compaction` 事件，并携带 `total_tokens` 与 `emergency: true`。

更新 `human_tests/readme.md` 中 Agent 内置命令测试用例数量。

## 校验要求

按顺序执行：

1. 单元测试：`cargo test -p bifrost-agent session::tests:: -- --nocapture`
2. 单元测试：`cargo test -p bifrost-agent compact::tests:: -- --nocapture`
3. E2E：`bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`
4. human_tests：按 `human_tests/agent-builtin-commands.md` 新增用例逐条执行并记录结果
5. `cargo fmt --all -- --check`
6. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
7. `cargo test --workspace --all-features`
8. 按修改范围执行 `bash scripts/ci/local-ci.sh --e2e-only shell`

## 文档更新要求

- 更新 `design/agent-context-status-compaction.md`
- 更新 `human_tests/agent-builtin-commands.md`
- 更新 `human_tests/readme.md`
