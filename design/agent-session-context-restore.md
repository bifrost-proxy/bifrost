# Agent Session Context Restore

## 功能模块描述

IM Gateway Agent 会把会话历史写入 JSONL，并在服务重启后按 `session_state.json` 中的 `history_path` 恢复同一 session。恢复时必须区分两类 token：

- `total_tokens_used`：历史所有模型调用的累计 API 消耗，只用于成本/总量展示和 goal accounting。
- `last_response_tokens`：最近一次模型请求返回的 context 快照，恢复后用于 `effective_token_count()` 和 Context 百分比。

同一恢复链路还必须避免把“恢复到内存的空闲会话”误报成“正在运行”：内存中可打开的 session 与正在执行 turn loop 的 session 是不同状态。外部 runner 的停止也必须同时写入 external-cli stop marker，不能只触发内置 Agent 的 cooperative stop signal。

## 实现逻辑

`load_session_runtime_state()` 从 JSONL 事件恢复 runtime state 时，继续使用 `scan_session_summary()` 计算累计 token，同时扫描最近的 `assistant_message.content.tokens` 作为 `last_response_tokens`。如果后续出现 `compaction` 事件且包含 `post_tokens`，则用压缩后的 `post_tokens` 覆盖最近响应快照。

所有恢复入口在设置 `session.total_tokens_used` 后调用 `session.restore_token_snapshot(runtime_state.last_response_tokens)`，把快照边界设为当前恢复后的 `history.len()`。这样下一条消息只会在最近 context 快照上追加新消息估算，不会把累计 token 当成当前 context。

`/agent/sessions/all` 继续用 `status:"active"` 表示 session 在内存中可打开，同时新增 `running` 与 `state` 字段：只有 `running:true` 才代表 turn loop 正在执行。正在执行的 session 会被 `AgentSessionManager::take_session()` 临时移出 idle session map，因此统一列表必须同时合并 `list_sessions()` 和 `list_active_turn_statuses()`。

`/stop` 的共享入口统一调用 `request_agent_stop()`：先请求内置 Agent stop signal，再按 session key 写入 external-cli stop marker。这样 IM 忙碌态 `/stop`、空闲态 `/stop` 和 `/agent/chat` `/stop` 对外部 runner 行为一致。

## 依赖项

- `crates/agent/src/persistence.rs`：JSONL runtime state 恢复。
- `crates/agent/src/session.rs`：`effective_token_count()` 与 token snapshot。
- `crates/bifrost-admin/src/handlers/im_gateway/*` 和 `agent_chat.rs`：IM/API 恢复入口。
- `crates/bifrost-admin/src/handlers/im_gateway/utils.rs`：共享 stop helper 与状态文本。
- `web/src/pages/AI/AgentChatSection.tsx`、`web/src/pages/Settings/tabs/agent/UnifiedSessionsSection.tsx`：根据 `running` 字段展示 Running/Active。
- `crates/bifrost-e2e/src/tests/im_gateway_session_persistence.rs`：重启恢复回归。

## 测试方案

- 单元测试：验证 runtime state 同时恢复累计 token 和最近 context 快照；验证 compaction `post_tokens` 优先；验证 `restore_token_snapshot()` 不让累计 token 进入 context。
- 单元测试：验证 running turn 状态与 idle session 列表分离；验证共享 `/stop` helper 可停止 external runner。
- E2E 测试：`im_gateway_agent_chat_restores_history_after_service_restart` 在重启恢复后先执行 `/status`，断言 Context 使用最近响应快照。
- UI 测试：Sessions 列表对 `running:false` 的 active session 展示 Active 而不是 Running。
- 真实场景测试：更新 `human_tests/agent-session-persistence.md` 和 `human_tests/im-gateway-external-cli-chat-gateway.md`，加入重启恢复 Context、状态误报和 external runner stop 回归用例并执行。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核恢复入口和 token 字段语义，运行受影响单元测试与 E2E，修复字段遗漏或断言问题。
- 第 2 轮：复查 diff、human_tests 索引和状态展示文案，复跑受影响验证命令，确认无需追加轮次。

## 校验要求

先执行 E2E，再执行 rust-project-validate 要求的 fmt、clippy、受影响测试、workspace all-features 测试。若 local-ci 因耗时未执行，最终验证矩阵必须说明风险。

## 文档更新要求

更新 `human_tests/agent-session-persistence.md` 和 `human_tests/readme.md`，记录本次 Context 恢复回归测试。
