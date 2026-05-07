# Agent 内置命令全面测试

## 功能模块说明

Agent 内置 11 个斜杠命令（/help、/clear、/reset、/undo、/compact、/status、/resume、/remember、/memories、/forget、/skill），通过 `POST /_bifrost/api/im-gateway/agent/chat` 或 IM 消息触发。本文档覆盖每个命令的正常路径、边界条件和错误处理。

## 前置条件

1. 启动 Bifrost 服务：
```bash
BIFROST_DATA_DIR=./.bifrost-cmd-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy
```
2. 确认 Agent 已启用（`GET /_bifrost/api/im-gateway/agent` 返回 `enabled: true`）
3. API 基础 URL：`http://127.0.0.1:8801/_bifrost/api/im-gateway/agent/chat`
4. 请求格式：`POST` + `Content-Type: application/json` + `{"message": "...", "session_key": "..."}`

## 测试用例

### TC-BC-01: /help 返回完整帮助信息

**操作步骤**：
1. 发送 `{"message": "/help", "session_key": "tc-bc-01"}`
2. 检查响应内容

**预期结果**：
- `success: true`
- response 包含"可用命令:"
- response 包含"内置命令:"
- response 列出所有 11 个内置命令：/clear、/compact、/forget、/help、/memories、/remember、/reset、/resume、/skill、/status、/undo
- 每个命令附带中文描述
- response 包含"提示: 直接输入文本即可与 AI 对话。"

### TC-BC-02: /status 显示会话状态

**操作步骤**：
1. 发送 `{"message": "/status", "session_key": "tc-bc-02"}`

**预期结果**：
- `success: true`
- response 包含"会话状态:"
- response 包含"工作路径:"
- response 包含"消息数: 0"（新会话）
- response 包含"估算 token"、"API 累计 token"、"压缩次数"、"历史版本"、"MCP 工具数"

### TC-BC-03: /clear 清除会话历史

**操作步骤**：
1. 发送 `{"message": "/clear", "session_key": "tc-bc-03"}`

**预期结果**：
- `success: true`
- response 为"会话已重置，可以开始新的对话。"

### TC-BC-04: /reset 行为与 /clear 一致

**操作步骤**：
1. 发送 `{"message": "/reset", "session_key": "tc-bc-04"}`

**预期结果**：
- `success: true`
- response 为"会话已重置，可以开始新的对话。"

### TC-BC-05: /undo 默认回退 1 轮

**操作步骤**：
1. 发送 `{"message": "/undo", "session_key": "tc-bc-05"}`

**预期结果**：
- `success: true`
- response 包含"已回退 1 轮对话"
- response 包含"当前历史:"和"条消息"

### TC-BC-06: /undo N 回退指定轮数

**操作步骤**：
1. 发送 `{"message": "/undo 3", "session_key": "tc-bc-06"}`

**预期结果**：
- `success: true`
- response 包含"已回退 3 轮对话"

### TC-BC-07: /compact 在历史太少时提示无需压缩

**操作步骤**：
1. 发送 `{"message": "/compact", "session_key": "tc-bc-07"}`（新会话，无历史）

**预期结果**：
- `success: true`
- response 为"历史消息太少，无需压缩。"

### TC-BC-08: /remember 保存长期记忆

**操作步骤**：
1. 发送 `{"message": "/remember 这是一条测试记忆", "session_key": "tc-bc-08"}`

**预期结果**：
- `success: true`
- response 包含"已记住长期记忆:"
- response 包含记忆 ID

### TC-BC-09: /remember 无参数提示用法

**操作步骤**：
1. 发送 `{"message": "/remember", "session_key": "tc-bc-09"}`

**预期结果**：
- `success: true`
- response 为"用法: /remember <text>"

### TC-BC-10: /memories 列出可见长期记忆

**操作步骤**：
1. 先发送 `/remember 测试记忆内容`（确保有记忆存在）
2. 发送 `{"message": "/memories", "session_key": "tc-bc-10"}`

**预期结果**：
- `success: true`
- response 包含"当前可见长期记忆文件条目:"或"当前 scope 没有长期记忆。"
- 如果之前有 remember 过内容，应能看到相关记忆条目

### TC-BC-11: /forget 无参数提示用法

**操作步骤**：
1. 发送 `{"message": "/forget", "session_key": "tc-bc-11"}`

**预期结果**：
- `success: true`
- response 为"用法: /forget <id|last>"

### TC-BC-12: /forget last 删除最近的记忆

**操作步骤**：
1. 先发送 `/remember 要删除的记忆`
2. 发送 `{"message": "/forget last", "session_key": "tc-bc-12"}`

**预期结果**：
- `success: true`
- response 包含"已忘记长期记忆:"或"没有找到可忘记的长期记忆。"

### TC-BC-13: /resume 在无历史时提示无记录

**操作步骤**：
1. 发送 `{"message": "/resume", "session_key": "completely-new-session-xyz"}`（从未使用过的 session key）

**预期结果**：
- `success: true`
- response 为"没有找到可恢复的会话记录。"

### TC-BC-14: /resume 恢复有历史的会话

**操作步骤**：
1. 先用 session_key="tc-bc-14" 发送至少一条普通消息或 `/remember` 命令
2. 重启服务（或等 session 过期）
3. 用相同 session_key 发送 `{"message": "/resume", "session_key": "tc-bc-14"}`

**预期结果**：
- `success: true`
- response 包含"已恢复会话历史，加载了"和"条消息"
- 消息数 > 0

### TC-BC-15: /skill 返回 Skill Creator 提示

**操作步骤**：
1. 发送 `{"message": "/skill", "session_key": "tc-bc-15"}`

**预期结果**：
- `success: true`
- response 为"Skill Creator 已启动。请描述要创建或编辑的 skill。"

### TC-BC-16: 未知命令返回错误提示

**操作步骤**：
1. 发送 `{"message": "/nonexistent_cmd", "session_key": "tc-bc-16"}`

**预期结果**：
- `success: true`
- response 为"未知命令: /nonexistent_cmd"

### TC-BC-17: Session-free 命令在 session 忙碌时立即响应

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息（如 "Tell me a story"），使用 session_key="tc-bc-17"
2. 在该请求处理期间（1 秒后），发送 `/help`，使用相同 session_key
3. 在该请求处理期间，发送 `/remember busy_test`，使用相同 session_key

**预期结果**：
- `/help` 立即返回帮助信息（不等待第一个请求完成）
- `/remember` 立即返回"已记住长期记忆: ..."

### TC-BC-18: Session-required 命令在 session 忙碌时返回忙碌提示

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息（如 "Tell me a story"），使用 session_key="tc-bc-18"
2. 在该请求处理期间（1 秒后），发送 `/status`，使用相同 session_key
3. 在该请求处理期间，发送 `/clear`，使用相同 session_key

**预期结果**：
- `/status` 返回"⏳ Agent 正在处理中，请稍后再试。"
- `/clear` 返回"⏳ Agent 正在处理中，请稍后再试。"
- 提示中包含 session-free 命令列表

### TC-BC-19: Session 忙碌结束后恢复正常

**操作步骤**：
1. 发送一条需要 LLM 处理的普通消息，使用 session_key="tc-bc-19"
2. 等待请求完成
3. 发送 `/status`，使用相同 session_key

**预期结果**：
- `/status` 正常返回会话状态信息（不再返回忙碌提示）

### TC-BC-20: /status 在 Agent Loop 运行中返回实时指标

**操作步骤**：
1. 使用临时数据目录启动 Bifrost：`BIFROST_DATA_DIR=./.bifrost-cmd-test cargo run --bin bifrost -- start -p 8801 --unsafe-ssl --no-system-proxy`
2. 将 Agent 配置为一个会延迟 3 秒返回的 mock Chat Completions provider。
3. 发送一条普通消息，使用 session_key="tc-bc-20"，保持请求仍在执行中。
4. 在第 3 步请求未完成时，发送 `{"message": "/status", "session_key": "tc-bc-20"}`。
5. 等待第 3 步请求完成，再次发送 `/status`。

**预期结果**：
- 运行中 `/status` 返回 `success: true`
- response 包含"会话状态:"、"正在处理中"、"工作路径:"、"Loop:"、"实时 token:"、"Context 用量:"、"压缩次数:"
- response 不再只是"Agent 正在处理中，请稍后再试。"
- JSON 响应包含 `active_status` 对象，且 `active_status.current_loop_iteration >= 1`
- `active_status.work_dir` 等于当前 session 的工作目录（未显式传入时允许为 `null`）
- `active_status.max_loop_iterations` 等于当前 Agent 配置的迭代上限
- 未显式配置 `model_context_window` 时，`active_status.context_window_tokens` 等于 `250000`
- `active_status.context_usage_percent` 为可读数值或 `null`（仅当未配置 context window 时允许为 `null`）
- 忙碌结束后的 `/status` 返回空闲会话状态，包含"API 累计 token"与"Context 用量"

### TC-BC-21: 回归 - 工具结果追加后 /status 展示当前上下文估算

**背景**：运行中的 Agent 在收到模型响应后会记录 `last_response_tokens`。此前工具结果追加到 `session.history` 后没有让该快照失效，可能导致 `/status` 的运行中状态和自动压缩判断继续参考旧模型响应 token，而不是工具结果追加后的当前上下文。

**操作步骤**：
1. 使用临时数据目录启动 Bifrost：`BIFROST_DATA_DIR=<temp_dir> ./target/debug/bifrost start --host 127.0.0.1 -p <non_9900_port> --unsafe-ssl --no-system-proxy`。
2. 将 Agent 配置为 mock Chat Completions provider。
3. mock 第一次响应返回 `exec_command` 工具调用，`usage.total_tokens = 17`，工具命令输出大体积文本。
4. mock 第二次模型请求延迟 3 秒返回最终文本。
5. 在第二次模型请求执行期间，发送同 session 的 `/status`。

**预期结果**：
- `/status` 返回 `success: true`，不是忙碌提示。
- JSON 响应包含 `active_status`。
- `active_status.current_loop_iteration == 2`。
- `active_status.estimated_context_tokens > 10000`，能反映大体积工具结果已经进入当前 history。
- `active_status.last_response_tokens == null`，说明工具结果追加后旧模型响应 token 快照已失效。
- response 文本中的 `Context 用量: ~<estimated_context_tokens> / 250000` 与 JSON 字段一致。
- response 文本中的 `实时 token` 仍展示累计 token，但最近响应显示 `N/A`，不把旧的 `17` 当作当前上下文。

**本次执行结果**：通过。2026-05-07 执行 `BIFROST_PORT=18897 MOCK_PORT=18898 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，脚本使用临时数据目录、非 9900 端口和 `--no-system-proxy` 启动当前源码版 Bifrost；运行中 `/status` 在第二轮模型请求期间返回 `active_status.current_loop_iteration == 2`、`estimated_context_tokens = 12061`（大于 10000）、`last_response_tokens = null`，文本中的 `Context 用量` 与 JSON 字段一致，最近响应显示 `N/A`；脚本输出 `[agent-builtin-status-runtime] PASS`。

### TC-BC-22: 回归 - 自动压缩判断不被较小的旧响应 token 遮蔽

**背景**：此前 `should_compact()` 优先读取 `last_response_tokens`。当 history 估算已超过阈值，而旧模型响应或压缩摘要模型请求的 token 较小时，自动压缩可能被跳过，出现 `Context 用量` 远超窗口但 `压缩次数` 增长很少的现象。

**操作步骤**：
1. 运行单元回归：`cargo test -p bifrost-agent compact::tests::test_should_compact_uses_history_estimate_when_response_snapshot_is_smaller -- --nocapture`。
2. 运行 session token 快照回归：`cargo test -p bifrost-agent session::tests::test_ -- --nocapture`。
3. 检查测试输出。

**预期结果**：
- `compact::tests::test_should_compact_uses_history_estimate_when_response_snapshot_is_smaller` 通过。
- session 模块测试通过，包含：
  - `test_background_token_usage_does_not_update_context_snapshot`
  - `test_history_growth_invalidates_last_response_tokens`
  - `test_effective_token_count_never_hides_larger_estimate`
- 证明较小的旧响应 token 不会遮蔽更大的 history 估算；压缩摘要调用只累计 token 消耗，不污染最近响应快照。

**本次执行结果**：通过。2026-05-07 执行 `cargo test -p bifrost-agent compact::tests::test_should_compact_uses_history_estimate_when_response_snapshot_is_smaller -- --nocapture`，结果 `1 passed`；执行 `cargo test -p bifrost-agent session::tests::test_ -- --nocapture`，结果 `57 passed`，其中包含 `test_background_token_usage_does_not_update_context_snapshot`、`test_history_growth_invalidates_last_response_tokens` 与 `test_effective_token_count_never_hides_larger_estimate`。

### TC-BC-23: 回归 - Emergency compaction 也记录完整压缩统计事件

**背景**：普通 `/compact`、pre-turn 与 mid-turn 自动压缩会写入 `compaction` 事件；此前 context window overflow 后的 emergency compaction 只增加内存中的 `compaction_count`，没有写入 recorder，导致会话事件流中的压缩次数统计不完备。

**操作步骤**：
1. 运行事件 metadata 回归：`cargo test -p bifrost-agent session::tests::test_record_compaction_event_includes_emergency_and_total_tokens -- --nocapture`。
2. 检查测试输出。

**预期结果**：
- 测试通过。
- 写入的 `compaction` 事件包含 `emergency: true`。
- 写入的 `compaction` 事件包含 `total_tokens`。
- 写入的 `compaction` 事件包含当前 `compaction_count`。
- 证明 emergency compaction 与 manual/pre-turn/mid-turn compaction 使用一致的 recorder metadata 口径。

**本次执行结果**：通过。2026-05-07 执行 `cargo test -p bifrost-agent session::tests::test_record_compaction_event_includes_emergency_and_total_tokens -- --nocapture`，结果 `1 passed`；事件内容断言覆盖 `emergency: true`、`total_tokens = 1234`、`compaction_count = 2` 与 `phase = "mid_turn"`。

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-cmd-test`
