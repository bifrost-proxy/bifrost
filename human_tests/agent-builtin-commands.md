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

**背景**：运行中的 Agent 在收到模型响应后会记录 `last_response_tokens`。上下文口径不是简单清空该快照，而是使用"最近 API usage + 最近模型 item 之后新增 items 的估算"。工具结果追加到 `session.history` 后，`/status` 和自动压缩判断必须展示同一套增量 context 口径。

**操作步骤**：
1. 使用临时数据目录启动 Bifrost：`BIFROST_DATA_DIR=<temp_dir> ./target/debug/bifrost start --host 127.0.0.1 -p <non_9900_port> --unsafe-ssl --no-system-proxy`。
2. 将 Agent 配置为 mock Chat Completions provider。
3. mock 第一次响应返回 `exec_command` 工具调用，`usage.total_tokens = 17`，工具命令输出大体积文本。
4. mock 第二次模型请求延迟 8 秒返回最终文本，保证 CI 高负载下 `/status` 轮询有稳定的运行中采样窗口。
5. 在第二次模型请求执行期间，发送同 session 的 `/status`。

**预期结果**：
- `/status` 返回 `success: true`，不是忙碌提示。
- JSON 响应包含 `active_status`。
- `active_status.current_loop_iteration == 2`。
- `active_status.estimated_context_tokens > 10017`，能反映 `17 + 大体积工具结果估算` 已进入当前 context 口径。
- `active_status.last_response_tokens == 17`，说明最近 API usage 被保留，并与新增 items 估算合并。
- response 文本中的 `Context 用量: ~<estimated_context_tokens> / 250000` 与 JSON 字段一致。
- response 文本中的 `实时 token` 仍展示累计 token，最近响应显示 `17`，`Context 用量` 不等于单独的 `17`。

**本次执行结果**：通过。2026-05-09 执行 `ADMIN_PORT=18121 MOCK_HTTP_PORT=18122 bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`，脚本使用临时数据目录、非 9900 端口和 `--no-system-proxy` 启动当前源码版 Bifrost；运行中 `/status` 在第二轮模型请求期间通过脚本断言：`active_status.current_loop_iteration == 2`、`estimated_context_tokens > 10017`、`last_response_tokens == 17`，文本中的 `Context 用量` 与 JSON 字段一致，最近响应显示 `17`；脚本输出 `[agent-builtin-status-runtime] PASS`。本次同时验证了 CI 高负载下的轮询加固：第二轮 mock 响应保留 8 秒采样窗口，脚本保存最后一次 `active_status` 响应用于最终断言，避免只捕获到 turn 完成后的空闲 `/status`。

### TC-BC-22: 回归 - 自动压缩判断不被较小的旧响应 token 遮蔽

**背景**：此前 `should_compact()` 无法表达 last API usage + appended items 口径。当前要求是保留最近 API usage，同时把最近模型 item 之后新增的 user/tool/guide/pending items 增量估算进去；压缩摘要模型调用只累计消耗，不污染最近普通响应快照。

**操作步骤**：
1. 运行单元回归：`cargo test -p bifrost-agent compact::tests::test_should_compact_adds_items_after_last_model_snapshot -- --nocapture`。
2. 运行 session token 快照回归：`cargo test -p bifrost-agent session::tests::test_history_growth_after_response_is_incrementally_accounted -- --nocapture`。
3. 运行模型边界回归：`cargo test -p bifrost-agent session::tests::test_model_message_advances_incremental_token_boundary -- --nocapture`。
3. 检查测试输出。

**预期结果**：
- `compact::tests::test_should_compact_adds_items_after_last_model_snapshot` 通过。
- session 模块测试通过，包含：
  - `test_background_token_usage_does_not_update_context_snapshot`
  - `test_history_growth_after_response_is_incrementally_accounted`
  - `test_model_message_advances_incremental_token_boundary`
- 证明普通响应 usage 被保留，新增 items 被增量计入；压缩摘要调用只累计 token 消耗，不污染最近响应快照。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，当前结果 `20 passed`，覆盖 `test_should_compact_adds_items_after_last_model_snapshot`；执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`，其中包含 `test_background_token_usage_does_not_update_context_snapshot`、`test_history_growth_after_response_is_incrementally_accounted` 与 `test_model_message_advances_incremental_token_boundary`。

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

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`；其中 `test_record_compaction_event_includes_emergency_and_total_tokens` 断言覆盖 `emergency: true`、`total_tokens = 1234`、`compaction_count = 2` 与 `phase = "mid_turn"`。

### TC-BC-24: 回归 - guide / pending queue 追加消息后继续 loop 前先执行 mid-turn compaction

**背景**：工具调用分支会在 tool result 和 guide message 追加到 `session.history` 后执行 mid-turn compaction；但无工具调用的最终响应分支在 turn-end guide message 或 `pending_messages` 追加后直接 `continue` 进入下一次模型请求，可能绕过压缩检查，让新增的大上下文直接进入下一轮请求。

**操作步骤**：
1. 运行回归测试：`cargo test -p bifrost-agent session::tests::test_queued_continuation_compacts_before_next_model_request -- --nocapture`。
2. 检查测试输出。

**预期结果**：
- 测试通过。
- 当 pending queue 追加的大消息使 history 估算超过阈值时，继续下一次模型请求前会执行一次 mid-turn compaction。
- `compaction_count` 增加，`history_version` 更新，token snapshot 在 history rewrite 后按 compacted history 与 base instructions 重算。
- mid-turn compaction 的 replacement history 包含 reinjected 非 system prompt context / memory，不包含 base instructions，并保持 summary 在末尾。
- 证明 guide / queue 继续 loop 场景与工具调用分支使用一致的自动压缩边界。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`；其中 `test_queued_continuation_compacts_before_next_model_request` 使用本地 mock Chat Completions provider 触发 mid-turn compaction，断言 `compaction_count == 1`、replacement history 包含非 system prompt context 与 memory context、不包含 base instructions，summary 位于 history 末尾，且 active status 中的 `compaction_count`、`history_version`、`estimated_context_tokens` 与当前 session 一致，压缩后 token snapshot 已包含 base instructions。补跑 `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`，脚本输出 `[im-guide-queue-human-api] PASS`，覆盖 turn-end guide、FIFO queue、guide 优先和空白忽略黑盒链路。

### TC-BC-25: 回归 - emergency compaction 与 trim retry 改写 history 后立即刷新运行中 /status

**背景**：context window overflow 后的 emergency compaction 和 fallback trim retry 会改写 `session.history`。此前 recorder 与内存统计已经更新，但 active `/status` 快照可能要等后续模型响应或下一轮循环才刷新，短窗口内显示旧的 `compaction_count`、`history_version` 或 context 估算。

**操作步骤**：
1. 运行 active status 刷新回归：`cargo test -p bifrost-agent session::tests::test_context_rewrite_refreshes_active_status_snapshot -- --nocapture`。
2. 运行 trim token 快照回归：`cargo test -p bifrost-agent session::tests::test_trim_oldest_messages_invalidates_response_tokens -- --nocapture`。
3. 检查测试输出。

**预期结果**：
- 两个测试均通过。
- history 被 compaction/trim 改写后，active status 中的 `compaction_count`、`history_version`、`estimated_context_tokens` 与当前 session 一致。
- trim retry 清空旧 `last_response_tokens`，避免旧响应 token 在 history 改写后继续参与上下文判断或展示。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`；其中 `test_context_rewrite_refreshes_active_status_snapshot` 断言 history 改写并刷新后 active status 与 session 一致，`test_trim_oldest_messages_invalidates_response_tokens` 断言 trim history 后 `last_response_tokens == null` 且 `history_version == 1`。

### TC-BC-26: 回归 - replacement history summary placement 与 initial context reinjection

**背景**：此前 Bifrost 压缩后的 replacement history 形状偏向 `[summary, recent...]`，且 mid-turn initial context 只注入轻量 reminder。local compaction 的核心形状是 `[recent user messages..., summary]`，mid-turn initial context 插在最后一个真实 user 前，使 summary 保持末尾边界。

**操作步骤**：
1. 运行 replacement history 形状回归：`cargo test -p bifrost-agent compact::tests::test_build_compacted_history_places_summary_after_recent_messages -- --nocapture`。
2. 运行 initial context 插入回归：`cargo test -p bifrost-agent compact::tests::test_insert_initial_context_before_last_real_user_message -- --nocapture`。
3. 运行 pending continuation mid-turn compaction 回归：`cargo test -p bifrost-agent session::tests::test_queued_continuation_compacts_before_next_model_request -- --nocapture`。

**预期结果**：
- 三个测试均通过。
- compacted history 的最后一条是 summary，recent message 保持在 summary 之前。
- initial context 插入到最后一个真实 user 之前，summary 仍保持末尾。
- mid-turn compaction 注入当前 turn 的非 system prompt context / memory，而不是旧的轻量 reminder，且 base instructions 不进入 replacement history。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，当前结果 `20 passed`，覆盖 summary-last 和 initial context 插入；执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`，覆盖 mid-turn 非 system initial context reinjection。

### TC-BC-27: 回归 - Codex local compaction 模板、纯文本 replacement history 与 token budget 对齐

**背景**：Codex local compaction 只收集真实 user messages，并在 replacement history 中重建为 text-only user messages，最后追加 `SUMMARY_PREFIX + "\n" + summary`。此前 Bifrost 虽然已经避免保留 assistant/tool/tool_result 后缀，但仍存在 prompt/prefix 非 Codex 原文、user carry-over 使用 char/byte 预算、以及 clone 整条 `ChatMessage` 可能保留 `content_parts` 的差异。

**操作步骤**：
1. 运行模板与 replacement history 回归：`cargo test -p bifrost-agent compact::tests:: -- --nocapture`。
2. 检查测试输出中以下用例通过：
   - `test_codex_compaction_templates_are_exact`
   - `test_collect_user_messages_caps_preserved_user_budget`
   - `test_build_compacted_history_rebuilds_text_only_user_messages`
   - `test_compaction_drops_tool_artifacts_from_replacement_messages`

**预期结果**：
- compaction prompt 与 summary prefix 与 Codex 模板逐字一致。
- summary message 以 `SUMMARY_PREFIX + "\n"` 开头，旧 summary 不会混入真实 user carry-over。
- user carry-over 使用 approximate token budget，超限时保留包含 `tokens truncated` 的中间截断文本。
- 压缩后的 replacement history 只包含 text-only user messages 和最后的 summary，不保留 assistant/tool/tool_result，也不保留图片 `content_parts`。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent compact::tests::test_codex_compaction_templates_are_exact -- --nocapture`，结果 `1 passed`；执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，当前结果 `20 passed`。其中 `test_codex_compaction_templates_are_exact` 验证 prompt/prefix 与 Codex 模板逐字一致：prompt 第一行是 `Create a handoff summary...`，不包含额外的 `concise`，且 prompt 末尾保留 Codex 文件中的换行；`test_collect_user_messages_caps_preserved_user_budget` 验证 token budget 与 `tokens truncated` 截断，`test_build_compacted_history_rebuilds_text_only_user_messages` 验证图片 user message 被重建为 text-only user message，`test_compaction_drops_tool_artifacts_from_replacement_messages` 验证 replacement history 不保留 assistant/tool/tool_result。

### TC-BC-28: 回归 - 压缩后 token snapshot 包含 base instructions

**背景**：Codex 安装 compacted history 后会重新估算 replacement history + base instructions。此前 Bifrost 只估算 history，导致 `/status` 和下一轮 `should_compact()` 低估 context。

**操作步骤**：
1. 运行 base instructions token snapshot 回归：
   `cargo test -p bifrost-agent session::tests::test_recompute_token_snapshot_includes_base_instructions -- --nocapture`。
2. 运行 queued continuation 正向自动压缩回归：
   `cargo test -p bifrost-agent session::tests::test_queued_continuation_compacts_before_next_model_request -- --nocapture`。

**预期结果**：
- 压缩后 `last_response_tokens` 等于 compacted history 估算值加 base instructions 估算值。
- 当 pending queue 追加大消息且下一步必须继续模型 loop 时，仍会执行 mid-turn compaction。

**本次执行结果**：通过。2026-05-08 依次执行 `cargo test -p bifrost-agent session::tests::test_recompute_token_snapshot_includes_base_instructions -- --nocapture`、`cargo test -p bifrost-agent session::tests::test_queued_continuation_compacts_before_next_model_request -- --nocapture`，两个用例均通过。验证了压缩后 token snapshot 包含 base instructions；pending continuation 需要继续模型 loop 时仍会执行 mid-turn compaction。

### TC-BC-29: 回归 - mid-turn compact 后 base instructions 不进 replacement history，非 system context / memory 不重复注入

**背景**：Bifrost mid-turn compaction 曾把完整 `prompt_prefix` 放进 replacement history，其中包含 system/base instructions；下一次 `build_messages()` 又会 prepend 同一份 `prompt_prefix`，同时 token snapshot 重算还会额外计入 base instructions。Codex local compaction 不把 base instructions 当 history item，replacement history 只携带非 system initial context，重算时 base instructions 只加一次。

**操作步骤**：
1. 运行 mid-turn initial context builder 回归：
   `cargo test -p bifrost-agent session::tests::test_mid_turn_initial_context_excludes_base_instructions -- --nocapture`。
2. 运行 model request 构造去重回归：
   `cargo test -p bifrost-agent session::tests::test_build_messages_dedupes_injected_mid_turn_context -- --nocapture`。
3. 运行 request history 裁剪下的去重边界回归：
   `cargo test -p bifrost-agent session::tests::test_build_messages_only_dedupes_context_selected_for_request -- --nocapture`。
4. 运行 pending continuation 正向自动压缩回归：
   `cargo test -p bifrost-agent session::tests::test_queued_continuation_compacts_before_next_model_request -- --nocapture`。

**预期结果**：
- 四个测试均通过。
- `build_mid_turn_initial_context()` 返回的 replacement context 不包含 `base instructions`，但包含 developer context、contextual user environment context 和 memory context。
- `build_messages()` 在 history 已携带这些非 system context / memory 时不会重复 prepend；下一次请求中 base instructions、developer context、environment context、memory context 各只出现一次。
- 当 `max_history` 裁剪使 reinjected context 不在本次请求选中的 history 中时，`build_messages()` 仍会 prepend prompt prefix，避免去重导致 context 缺失。
- `test_queued_continuation_compacts_before_next_model_request` 同时断言 compacted history 中没有 base instructions，token snapshot 等于 compacted history 估算值加一次 base instructions 估算值，active status 与 session 当前值一致。

**本次执行结果**：通过。2026-05-08 已执行操作步骤中的命令，结果均为 `1 passed`；确认 mid-turn compact 后 replacement history 不包含 base instructions，下一次 model request 不重复注入非 system context / memory，token snapshot 只额外计入一次 base instructions，并且 max_history 裁剪不会误跳过本次请求实际缺失的 prompt context。

### TC-BC-30: 回归 - 普通 turn 采样前按 Codex 当前策略执行 PreTurn DoNotInject 自动压缩

**背景**：当前 Codex `run_turn()` 在采样前会执行 `run_pre_sampling_compact()`；当上一轮累计 token usage 达到 auto compact limit 时，使用 `InitialContextInjection::DoNotInject` 和 `CompactionPhase::PreTurn` 先压缩旧 history，然后才记录本轮 context updates / user input。Bifrost 不能完全移除这条路径，否则普通新 turn 会在已有 history 超阈值时直接进入模型请求。

**操作步骤**：
1. 运行 pre-sampling compact 回归：
   `cargo test -p bifrost-agent session::tests::test_pre_sampling_auto_compacts_before_model_request -- --nocapture`。
2. 运行 session 全量回归：
   `cargo test -p bifrost-agent session::tests:: -- --nocapture`。

**预期结果**：
- 两个测试均通过。
- 当旧 history 已超过 auto compact limit 时，普通 turn 在发送模型请求前先执行一次 compaction，`compaction_count == 1`。
- pre-sampling compaction 使用 `DoNotInject`，replacement history 不包含 base instructions，也不会插入 developer/contextual user/memory。
- 本轮新 user message 在 compaction 之后才进入 history，并参与随后的模型请求；最终 response 来自第二次模型调用，不是 compaction summary。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent session::tests::test_pre_sampling_auto_compacts_before_model_request -- --nocapture`，结果 `1 passed`；执行 `cargo test -p bifrost-agent session::tests:: -- --nocapture`，结果 `68 passed`。验证普通 turn 会在采样前按 Codex 当前策略执行 `PreTurn + DoNotInject` 自动压缩，并且本轮 user message 保留在压缩后的真实请求历史中。

### TC-BC-31: 回归 - compaction summary 生成请求使用 Codex local structured history 形态

**背景**：Codex local compaction 会把 compact prompt 作为新的 user input 追加到 cloned structured history，并通过 `Prompt.base_instructions` 单独携带 base instructions。Bifrost 曾把完整 history 格式化为 `[role]: ...` 单条 user message，并把 compaction prompt 放在 system message 中，导致生成 summary 的输入形态和 Codex local compaction 不一致。

**操作步骤**：
1. 运行 compaction request shape 回归：
   `cargo test -p bifrost-agent compact::tests::test_build_compaction_messages_uses_codex_local_request_shape -- --nocapture`。
2. 运行 empty base instructions 边界回归：
   `cargo test -p bifrost-agent compact::tests::test_build_compaction_messages_omits_empty_base_instructions -- --nocapture`。
3. 运行 compact 模块全量回归：
   `cargo test -p bifrost-agent compact::tests:: -- --nocapture`。

**预期结果**：
- 三个命令均通过。
- summary 生成请求保留原始 structured history，包括 assistant tool calls 和 tool result。
- `COMPACTION_PROMPT` 作为最后一条 `role=user` 消息发送，不作为 system message 发送。
- 有效 base instructions 只作为请求开头的 system item 携带；空白 base instructions 不注入。
- 请求内容中不再出现 `[user]: ...` / `[assistant]: ...` 这类扁平化 history 文本。

**本次执行结果**：通过。2026-05-08 依次执行 `cargo test -p bifrost-agent compact::tests::test_build_compaction_messages_uses_codex_local_request_shape -- --nocapture`、`cargo test -p bifrost-agent compact::tests::test_build_compaction_messages_omits_empty_base_instructions -- --nocapture`，两个用例均为 `1 passed`；执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，当前结果 `20 passed`。验证 summary 生成请求现在保留 structured history，最后追加 `role=user` 的 `COMPACTION_PROMPT`，不再把 prompt 放入 system message，也不再发送 `[user]: ...` / `[assistant]: ...` 扁平化文本；有效 base instructions 只在请求开头作为 system item 携带，空白 base instructions 不注入。

### TC-BC-32: 回归 - compaction 请求超上下文后移除最老 history item 并重试

**背景**：Codex local compaction 在 summary 生成请求本身遇到 `ContextWindowExceeded` 时，会从 cloned history 中移除最老 item 并重试，直到只剩 compact prompt 或请求成功。Bifrost 改为 structured history 请求后，如果没有这条闭环，长历史会让 compaction summary 生成直接失败。

**操作步骤**：
1. 运行请求副本裁剪边界回归：
   `cargo test -p bifrost-agent compact::tests::test_remove_oldest_history_item_preserves_base_and_compaction_prompt -- --nocapture`。
2. 运行真实 compact_session retry 回归：
   `cargo test -p bifrost-agent compact::tests::test_compaction_retries_context_window_error_by_dropping_oldest_request_item -- --nocapture`。
3. 运行 compact 模块全量回归：
   `cargo test -p bifrost-agent compact::tests:: -- --nocapture`。

**预期结果**：
- 三个命令均通过。
- 第一次 summary 请求返回 `context_length_exceeded` 后，Bifrost 只从 compaction 请求副本中移除最老 history item 后重试。
- retry 请求仍保留开头的 base/system instructions 和末尾的 `COMPACTION_PROMPT`。
- session 原始 history 不因 summary 请求重试被裁剪；最终 replacement history 仍按真实 user messages + summary 构造。

**本次执行结果**：通过。2026-05-08 依次执行 `cargo test -p bifrost-agent compact::tests::test_remove_oldest_history_item_preserves_base_and_compaction_prompt -- --nocapture`、`cargo test -p bifrost-agent compact::tests::test_compaction_retries_context_window_error_by_dropping_oldest_request_item -- --nocapture`，两个用例均为 `1 passed`；执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，结果 `20 passed`。真实 `compact_session` 回归中 mock provider 第一次返回 `context_length_exceeded`，第二次返回 summary；断言 retry 请求移除了最老的 `oldest user`，仍保留开头的 `base instructions`、后续 structured history item 和末尾 `COMPACTION_PROMPT`，并且最终 session replacement history 正常安装 summary。

### TC-BC-33: 回归 - compaction summary 请求 transient error 按 provider retry budget 重试

**背景**：Codex local compaction 在 summary 生成请求遇到非 `ContextWindowExceeded` 的 transient error 时，会按 provider `stream_max_retries()` 预算退避重试。Bifrost 曾经在 `compact_session()` 中对这类错误直接返回失败，导致 429/5xx/timeout/connection reset 会让 compaction 失败。

**操作步骤**：
1. 运行 transient retry 回归：
   `cargo test -p bifrost-agent compact::tests::test_compaction_retries_transient_error_using_provider_budget -- --nocapture`。
2. 运行 compact 模块全量回归：
   `cargo test -p bifrost-agent compact::tests:: -- --nocapture`。

**预期结果**：
- 两个命令均通过。
- mock provider 第一次返回 `500 temporary server error` 后，Bifrost 不裁剪 structured history，而是根据测试 provider 的 `stream_max_retries = 1` 重试 summary 请求。
- 第二次 summary 请求成功后，replacement history 正常安装 summary，`session.compaction_count == 1`。
- retry 请求仍保留 base/system instructions、原始 structured history 和末尾 `COMPACTION_PROMPT`。

**本次执行结果**：通过。2026-05-08 执行 `cargo test -p bifrost-agent compact::tests::test_compaction_retries_transient_error_using_provider_budget -- --nocapture`，结果 `1 passed`；执行 `cargo test -p bifrost-agent compact::tests:: -- --nocapture`，结果 `20 passed`。真实 `compact_session` 回归中 mock provider 第一次返回 `500 temporary server error`，第二次返回 summary；断言发生两次 summary 请求，第二次请求未裁剪 history，仍保留 `base instructions`、真实 user message 和末尾 `COMPACTION_PROMPT`，最终 session replacement history 正常安装 summary。

## 清理步骤

1. 停止 Bifrost 服务
2. 删除临时数据目录：`rm -rf ./.bifrost-cmd-test`
