# Agent Context Status 与自动压缩修复

## 功能模块说明

本模块覆盖 Bifrost Agent turn loop 中的 context compaction、initial context reinjection、token accounting、overflow degradation、运行中 `/status` 展示，以及会话持久化中的 compaction 统计事件。

本轮修复目标是完善核心语义：压缩后 replacement history 的末尾边界、mid-turn initial context reinjection 位置、自动压缩触发的 token 口径、guide / pending continuation 的压缩边界，以及 history rewrite 后的 active status 同步。

2026-05-08 补充修复目标是把 Bifrost local compaction 与 `/Users/eden/work/github/codex` 的 Codex local compaction 策略继续对齐，重点处理压缩后保留内容、summary 模板、summary 生成请求形态、user message 预算、token snapshot 重算和自动压缩触发时机的细节差异。

## 现状核查与根因

相关代码路径：

- `crates/agent/src/compact.rs`
  - `compact_session()` 构造 replacement history。
  - `collect_recent_user_messages()` 选择压缩后保留的近期用户消息。
  - `insert_initial_context_before_last_user_message()` 控制 mid-turn reinjection 位置。
- `crates/agent/src/session.rs`
  - `AgentSession::track_token_usage()` / `effective_token_count()` 维护压缩触发口径。
  - `run_turn_with_mcp_multimodal()` 控制 mid-turn、guide / pending continuation、overflow retry。
  - `record_compaction_event()` 写入持久化统计。
- `crates/agent/src/session_status.rs`
  - `refresh_active_turn_status()` 将 session state 投影到运行中 `/status`。
- `crates/agent/src/persistence.rs`
  - `ConversationRecorder::record_compaction()` 持久化 compaction 事件。

审计确认的根因：

1. Bifrost replacement history 仍是 `[summary, recent...]`，而 local compaction 应该是 `[recent user messages..., summary]`。这会改变压缩后下一次模型请求的最近上下文边界。
2. Bifrost mid-turn compaction 只注入轻量 reminder，仍依赖 `build_messages()` 每次 prepend prompt prefix；应该把 Codex local history 中的非 system initial context 重新插入 replacement history 边界。
3. guide / pending queue continuation 曾经在追加消息后直接进入下一 loop，绕过部分 mid-turn compaction 检查。
4. token accounting 只保存 `last_response_tokens` 或退回 full-history estimate，不能表达"last API usage + last model item 之后新增 items 估算"。
5. Bifrost 在普通 sampling overflow 后保留 emergency compact + trim retry，这是 Bifrost 的特殊保护，不是普通 sampling overflow 语义；必须收窄边界并明确记录。
6. history rewrite 后 active `/status` 曾存在短窗口旧快照，`compaction_count`、`history_version`、context token 口径可能和实际 session 不一致。
7. 后续复核发现 Bifrost 仍保留了若干非 Codex local compaction 细节：summary prompt/prefix 不是 Codex 原文；summary 生成请求把完整 history 扁平化为 `[role]: ...` 单条 user message，且把 compaction prompt 放在 system message；user carry-over 用 char/byte 预算而非 token 预算；replacement history 直接 clone `ChatMessage`，可能保留 `content_parts`；压缩后 token snapshot 没有把 base instructions 算入；普通新 turn 采样前缺少 Codex 当前仍保留的 pre-sampling compaction。

## 实现逻辑

### 1. replacement history 改为 summary-last

`compact_session()` 现在按 Codex local compaction 构造：

```text
[...recent_user_messages, summary_message]
```

`collect_user_messages()` 只返回真实 user message 的纯文本内容，过滤历史 compaction summary 和 Bifrost prompt-prefix 产生的 contextual user sections。`build_compacted_history()` 再用这些字符串重建 text-only `ChatMessage::user`，因此不会把 assistant/tool/tool_result 原样搬进 replacement history，也不会把多模态 `content_parts` 或图片内容继续留在压缩后的 carry-over 中。

user message carry-over 预算使用 Codex 相同的 `20_000` approximate tokens。预算从最新 user message 向前选择；遇到超出预算的单条 user message 时，使用 Codex 风格的 middle truncation marker（`tokens truncated`）截断后保留。

summary message 使用 Codex 原文模板：

```text
{SUMMARY_PREFIX}\n{summary}
```

`is_summary_message()` 也按 Codex 的 `SUMMARY_PREFIX + "\n"` 边界识别，避免普通用户消息仅以前缀文本开头时被误判。

这是按 local compaction 语义实现的部分。

### 2. mid-turn initial context reinjection 不包含 base instructions

`build_mid_turn_initial_context()` 不再只创建轻量 reminder。mid-turn compaction 会接收当前 turn 已解析的 `prompt_prefix` 和 optional `memory_message`，但只把非 system 的 concrete initial context items 插入 compacted history。

base/system instructions 对齐 Codex 的 `Prompt.base_instructions` 语义：它们不作为 history item 进入 replacement history，只在模型请求构造阶段 prepend，并在 token snapshot 重算时额外计入一次。

插入位置遵循以下规则：

1. 优先插入到最后一个真实用户消息之前。
2. 如果只剩 summary，则插入到 summary 之前。
3. 如果没有 user-like item，则追加。

Bifrost 仍保留自己的 prompt-prefix 架构：常规 `build_messages()` 会继续 prepend prompt prefix，但会跳过 replacement history 中已经存在的等价非 system context / memory message，避免 mid-turn compaction 后下一次请求重复看到 developer、contextual user 或 memory。system/base message 永远不通过 history 去重，因为它不应进入 replacement history。

### 2.1 summary 生成请求保持结构化 history

`compact_session()` 生成 handoff summary 时不再把 session history 格式化成 `[role]: ...` 文本，也不再把 `COMPACTION_PROMPT` 作为 system message 发送。请求构造改为 Codex local compaction 形态：

```text
[system(base_instructions if any), ...structured_history, user(COMPACTION_PROMPT)]
```

这保留 assistant tool calls、tool results、多模态 user message 等原始结构化消息形态，让 summary 模型看到的输入边界与正常模型请求一致。base instructions 只作为本次请求的 system item 参与，不写入 compacted replacement history；replacement history 仍由真实 user messages 与 summary 重建。

当 summary 生成请求自身遇到 context-window overflow 时，`compact_session()` 会对该请求副本执行 Codex-style retry：保留开头的 base/system instructions 和末尾的 `COMPACTION_PROMPT`，只移除 structured history 中最老的 item 后重试。这个裁剪只作用于 summary 请求副本，不修改 `session.history`；最终 replacement history 仍从真实 session history 收集 user messages。

当 summary 生成请求遇到非 context-window 的 transient error（例如 429、5xx、timeout、connection reset）时，`compact_session()` 现在按 provider 合并后的 `stream_max_retries` 预算做 exponential backoff retry。context-window retry 仍优先走移除最老 history item 的路径，并在裁剪后重置 transient retry 计数，保持和 Codex compact loop 相同的重试边界。

### 3. token accounting 改为 last usage + appended items

`AgentSession` 新增 `last_response_history_len`：

- 普通模型响应写入 `last_response_tokens`，并记录当时的 history 边界。
- assistant 文本 / tool_calls 写入 history 后，如果它正是刚返回的模型 item，则边界前移到该模型 item 之后。
- 后续 tool result、guide message、pending message、新用户消息不会清空 `last_response_tokens`，而是作为“last model item 之后新增 items”参与估算。
- compaction、rollback、clear、trim 等 history rewrite 会清空 `last_response_tokens` 和 `last_response_history_len`。

`effective_token_count()` 现在返回：

```text
last_response_tokens + estimate(history[last_response_history_len..])
```

当没有有效 API snapshot 时才退回 `estimate_tokens()`。`compact::should_compact()`、session list/detail 的 `estimated_tokens`、运行中 `/status.active_status.estimated_context_tokens` 都使用这同一口径。

这是按 token trigger 语义实现的部分。

### 4. guide / pending continuation 共用 mid-turn compaction 边界

抽取 `compact_mid_turn_if_needed()`，并在以下继续 loop 路径复用：

- tool result 和 mid-turn guide message 处理完成后；
- turn-end guide message 注入后；
- `pending_messages` 弹出并追加到 history 后。

这样新增 queued/guide input 后，如果下一步仍要进入模型请求，会先走同一套 `should_compact()` 和 `record_compaction_event()`，避免“刚追加大消息就直接进入下一次请求”的抖动。

### 5. emergency overflow 保留 Bifrost 特殊保护但收窄边界

普通 sampling overflow 不会主动改写 history 后重试；Bifrost 仍保留 emergency compact + retry + trim loop，原因是当前 IM/API runtime 更偏向长任务不中断，直接失败会丢掉已完成工具结果。

本轮对该特殊行为加边界：

- emergency compaction 使用与 mid-turn compaction 相同的 summary-last history 和非 system initial context reinjection。
- compaction 成功后立即刷新 active status。
- emergency compaction 写入统一 `compaction` 事件，并带 `emergency: true`。
- trim retry 每次裁剪后清空 token snapshot、递增 `history_version`、刷新 active status。

这是保留 Bifrost 特殊行为但明确边界的部分。

### 6. status 与 persistence 同步

`refresh_active_turn_status()` 使用 `session.effective_token_count()`，与 compaction trigger 口径一致。所有 compaction 路径通过 `record_compaction_event()` 记录：

- `trigger`、`reason`、`phase`
- `pre_tokens`、`post_tokens`、`tokens_saved`、`messages_removed`
- `compaction_count`
- `total_tokens`
- emergency 路径额外包含 `emergency: true`

history rewrite 后同步刷新 active status，避免 `/status` 展示旧 `history_version` 或旧 context token。

### 7. 压缩后 token snapshot 纳入 base instructions

Codex 的 `recompute_token_usage()` 会在 replacement history 上额外估算 base instructions。Bifrost 的 Chat Completions 架构虽然把 prompt prefix 作为消息 prepend，但压缩后 `last_response_tokens` 也必须包含 base/system instructions，否则 `/status` 和下一轮 `should_compact()` 会低估 context。

`compact_session()` 现在接收当前 turn 的 base instructions，并在安装 replacement history 后调用：

```text
estimate(history) + estimate(system(base_instructions))
```

手动 `/compact` 使用当前配置解析出的 base instructions；mid-turn / emergency compaction 使用本 turn 已解析的 base instructions。

### 8. 自动压缩触发时机对齐 Codex pre-sampling 与 post-sampling follow-up

Bifrost 在普通 turn 采样前恢复 Codex 当前的 pre-sampling compaction：解析 base instructions 与 prompt prefix 后、把本轮新 user message 写入 history 之前，如果上一轮累计 token usage 已达到 `auto_compact_token_limit`，执行：

```text
CompactionTrigger::Auto
CompactionReason::ContextLimit
CompactionPhase::PreTurn
InitialContextInjection::DoNotInject
```

该路径不注入 initial context，压缩后仍由正常 `build_messages()` 在随后模型请求里 prepend prompt prefix。这样新 user message 不会被卷入 pre-sampling compaction summary，base instructions 也不会进入 replacement history。

自动压缩还保留在会继续进入下一次模型请求的路径上：

- tool result / guide message 处理后继续 loop；
- turn-end guide 或 pending queue 追加后继续 loop；
- context overflow emergency fallback。

post-sampling 路径仍使用 mid-turn `BeforeLastUserMessage` 注入非 system initial context，与 Codex 的 `token_limit_reached && needs_follow_up` 语义保持一致。

## 依赖项

- `crates/agent/src/session.rs`
- `crates/agent/src/compact.rs`
- `crates/agent/src/session_status.rs`
- `crates/agent/src/persistence.rs`
- `crates/bifrost-admin/src/state.rs`（测试隔离修正，避免并发测试中默认 `RulesStorage` 被全局 env 污染）
- `e2e-tests/tests/test_agent_builtin_status_runtime.sh`
- `e2e-tests/tests/test_im_guide_queue_human_api.sh`
- `human_tests/agent-builtin-commands.md`
- `human_tests/readme.md`

## 测试方案

### 单元测试

1. `compact::tests::test_build_compacted_history_places_summary_after_recent_messages`：验证 replacement history 是 recent messages 在前、summary 在末尾。
2. `compact::tests::test_insert_initial_context_before_last_real_user_message`：验证 initial context 插入最后一个真实 user 前，summary 仍保持在末尾。
3. `compact::tests::test_should_compact_adds_items_after_last_model_snapshot`：验证压缩触发使用 last usage + appended items。
4. `session::tests::test_history_growth_after_response_is_incrementally_accounted`：验证新增用户消息不会清空 snapshot，而是增量计入 context。
5. `session::tests::test_model_message_advances_incremental_token_boundary`：验证 assistant model item 会推进 last model boundary。
6. `session::tests::test_queued_continuation_compacts_before_next_model_request`：验证 pending continuation 追加大消息后会先 compact，并且 reinjected context 包含非 system prompt context / memory，不包含 base instructions。
7. `session::tests::test_context_rewrite_refreshes_active_status_snapshot`：验证 history rewrite 后 active status 与 session 同步。
8. `session::tests::test_trim_oldest_messages_invalidates_response_tokens`：验证 trim retry 清空 token snapshot。
9. `session::tests::test_record_compaction_event_includes_emergency_and_total_tokens`：验证 emergency compaction 事件统计字段。
10. `session_status::tests::test_active_turn_status_text_contains_runtime_metrics`：验证 `/status` 文案字段。
11. `compact::tests::test_codex_compaction_templates_are_exact`：验证 compaction prompt 与 summary prefix 与 Codex 模板逐字一致。
12. `compact::tests::test_build_compaction_messages_uses_codex_local_request_shape`：验证 summary 生成请求保留 structured history，把 compaction prompt 作为最后一条 user message，且不再出现 `[role]: ...` 扁平化输入。
13. `compact::tests::test_compaction_retries_context_window_error_by_dropping_oldest_request_item`：验证 summary 请求超上下文后，移除请求副本中的最老 history item 并重试，且保留 base/system 与 compact prompt。
14. `compact::tests::test_compaction_retries_transient_error_using_provider_budget`：验证 summary 请求遇到 transient 500 时，按 provider `stream_max_retries` 预算重试且不裁剪 history。
15. `compact::tests::test_collect_user_messages_caps_preserved_user_budget`：验证 user carry-over 使用 token 预算并产生 `tokens truncated` marker。
16. `compact::tests::test_build_compacted_history_rebuilds_text_only_user_messages`：验证多模态 user message 压缩后重建为 text-only user message。
16. `session::tests::test_recompute_token_snapshot_includes_base_instructions`：验证压缩后 token snapshot 包含 base instructions。
17. `session::tests::test_pre_sampling_auto_compacts_before_model_request`：验证普通 turn 采样前如果历史已超过阈值，会用 `DoNotInject` 先压缩旧 history，再追加本轮 user message 并请求模型。
18. `session::tests::test_mid_turn_initial_context_excludes_base_instructions`：验证 mid-turn initial context builder 不把 base/system instructions 放进 replacement history。
19. `session::tests::test_build_messages_dedupes_injected_mid_turn_context`：验证 build_messages 在本次请求选中的 history 已携带非 system context / memory 时不会重复 prepend，但 system/base 仍只出现一次。
20. `session::tests::test_build_messages_only_dedupes_context_selected_for_request`：验证 max_history 裁剪掉 reinjected context 时，build_messages 不会误以完整 history 中存在该 context 为由跳过 prompt prefix。

### E2E 测试

更新并执行 `e2e-tests/tests/test_agent_builtin_status_runtime.sh`：

1. mock provider 返回较小 `usage.total_tokens = 17`。
2. 工具调用产生大体积输出并进入第二轮模型请求。
3. 运行中 `/status` 断言：
   - `active_status.current_loop_iteration == 2`
   - `active_status.last_response_tokens == 17`
   - `active_status.estimated_context_tokens > 10017`
   - 文本 `Context 用量` 与 JSON 字段一致
   - 文本最近响应显示 `17`

由于本轮触达 guide / pending continuation，补跑 `e2e-tests/tests/test_im_guide_queue_human_api.sh`，验证 turn-end guide、FIFO queue、guide 优先级和空白忽略仍符合预期。

### 真实场景测试（human_tests）

更新 `human_tests/agent-builtin-commands.md`：

1. 新增 `TC-BC-21`：运行中 `/status` 在工具结果追加后展示 last API usage + 新增 items 的 context 口径。
2. 新增 `TC-BC-22`：自动压缩判断使用增量 token accounting，不被旧 snapshot 遮蔽。
3. 新增 `TC-BC-23`：emergency compaction 记录完整统计事件。
4. 新增 `TC-BC-24`：guide / pending queue 追加消息后继续 loop 前先执行 mid-turn compaction。
5. 新增 `TC-BC-25`：emergency compaction 与 trim retry 改写 history 后立即刷新运行中 `/status`。
6. 新增 `TC-BC-27`：Codex local compaction 模板、纯文本 replacement history 与 token budget 对齐。
7. 新增 `TC-BC-28`：压缩后 token snapshot 包含 base instructions。
8. 新增 `TC-BC-29`：mid-turn compact 后 replacement history 不包含 base instructions，build_messages 不重复注入非 system context / memory，token snapshot 只额外计入一次 base instructions。
9. 新增 `TC-BC-30`：普通 turn pre-sampling auto compaction 对齐 Codex 当前 `DoNotInject + PreTurn` 路径。
10. 新增 `TC-BC-31`：summary 生成请求使用 Codex local structured history 形态，把 `COMPACTION_PROMPT` 作为最后一条 user message。
11. 新增 `TC-BC-32`：summary 请求超上下文后移除请求副本中的最老 history item 并重试。
12. 新增 `TC-BC-33`：summary 请求 transient error 按 provider retry budget 退避重试。

同步更新 `human_tests/readme.md` 中 Agent 内置命令测试用例数量。

## 校验要求

按顺序执行：

1. `cargo test -p bifrost-agent compact::tests:: -- --nocapture`
2. `cargo test -p bifrost-agent session::tests:: -- --nocapture`
3. `cargo test -p bifrost-agent session_status::tests:: -- --nocapture`
4. `bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`
5. `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`
6. 按 `human_tests/agent-builtin-commands.md` 新增/受影响用例逐条执行并记录结果
7. `cargo fmt --all -- --check`
8. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
9. `cargo test --workspace --all-features`
10. 按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`，并用上面的 targeted E2E 覆盖受影响链路。

## 本轮验证结果（2026-05-08）

已执行并通过：

1. `cargo test -p bifrost-agent compact::tests:: -- --nocapture`
   - 结果：20 passed
2. `cargo test -p bifrost-agent session::tests:: -- --nocapture`
   - 结果：68 passed
3. `cargo test -p bifrost-agent session_status::tests:: -- --nocapture`
   - 结果：2 passed
4. `bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`
   - 结果：`[agent-builtin-status-runtime] PASS`
   - 使用临时 `BIFROST_DATA_DIR`、非 9900 端口和 `--no-system-proxy`
5. `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`
   - 结果：`[im-guide-queue-human-api] PASS`
   - 使用临时 `BIFROST_DATA_DIR`、非 9900 端口和 `--no-system-proxy`
6. human_tests 新增/受影响用例逐条执行
   - 文档：`human_tests/agent-builtin-commands.md`
   - 用例：TC-BC-21、TC-BC-22、TC-BC-24、TC-BC-25、TC-BC-26、TC-BC-27、TC-BC-28、TC-BC-29、TC-BC-30、TC-BC-31、TC-BC-32、TC-BC-33
   - 结果：全部通过，实际结果已写回用例文档；`human_tests/readme.md` 索引已同步
7. `cargo fmt --all -- --check`
   - 结果：通过
8. `cargo clippy -p bifrost-agent --all-targets --all-features -- -D warnings`
   - 结果：通过
9. `cargo test -p bifrost-agent --all-features`
   - 结果：lib `566 passed`，P1 tools E2E `7 passed`，session skills integration `6 passed`，doc-test `1 passed`
10. `cargo test --workspace --all-features`
    - 结果：当前工作区仍被既有 traffic/port 脏改阻塞。最新失败点为 `crates/bifrost-e2e/src/tests/traffic_cli.rs:55` 初始化 `TrafficListOptions` 时缺少新增字段 `listener_port`。该阻塞不在本模块修改范围内，未回滚或改写用户的 traffic/port 脏改。
11. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
    - 结果：当前同样被 `crates/bifrost-e2e/src/tests/traffic_cli.rs:55` 阻塞，`bifrost_cli::commands::TrafficListOptions` 初始化缺少 `listener_port`。
12. `cargo test -p bifrost-admin state::tests::group_name_cache_ -- --nocapture`
    - 结果：4 passed
    - 说明：首次默认并发 `cargo test --workspace --all-features` 暴露出 group cache 测试先调用 `AdminState::new()` 再覆盖临时 `RulesStorage` 的全局 env 竞态；已改为 `AdminState::new_for_test()`。当前最新 workspace 全量验证另被 proxy 脏改编译错误阻塞，见第 10 项。

## 文档更新要求

- 更新 `design/agent-context-status-compaction.md`
- 更新 `human_tests/agent-builtin-commands.md`
- 更新 `human_tests/readme.md`
