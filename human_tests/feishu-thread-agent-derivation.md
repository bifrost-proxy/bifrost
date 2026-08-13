# 飞书群话题 Agent 派生真实场景测试

## 功能模块说明

验证飞书群聊话题从卡片/普通消息派生独立 Agent Session，覆盖本地 checkpoint、跨设备等价降级、多 Bot 隔离、命令优先路由、严格双消息上下文、话题内回复与 Runner 能力边界。

## 前置条件

1. 在仓库根目录构建 debug 二进制：`cargo build -p bifrost-cli --bin bifrost`。
2. 不复用用户正在运行的 Bifrost 数据目录；测试脚本创建独立临时目录、随机端口和 fake Feishu OpenAPI。
3. 使用 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_group_session_context.sh`。
4. 当前机器无 Claude Code；Claude 项只验证明确的 unavailable capability，不声称执行了 Claude runtime。

## 测试用例

### TC-FTD-01：缺本地锚点按普通消息派生

操作步骤：

1. 启动隔离服务和 fake Feishu OpenAPI。
2. 注入由另一 Bot 发送的 `card-parent` 卡片。
3. 在 `topic-cross-device` 话题先注入未提及当前 Bot 的回复，再注入 `@当前 Bot 基于这条卡片继续`。
4. 检查 Runner prompt、SQLite binding 和 session idle 状态。

预期结果：未 @ 的消息不触发；@ 消息创建 `im:feishu-group-e2e:group:chat-alpha:thread:topic-cross-device`；binding 的 `source_kind=message_context`；prompt 仅含根卡片可见文本与当前消息，不含此前群历史。

### TC-FTD-02：群主上下文不累计话题消息

操作步骤：在 TC-FTD-01 后执行下一次主群触发并检查 prompt/SQLite 计数。

预期结果：主群 prompt 不包含话题回复；话题消息仍在审计账本中，但不进入 `im_group_turns` 的上下文范围。

### TC-FTD-03：多 Bot / 跨设备归属边界

操作步骤：在同一群配置 provider A/B，分别注入广播、提及人类和显式提及 Bot B 的消息；检查两 provider 的消息账本。

预期结果：显式 Bot B 消息只由 B 认领；A 无本地锚点时不会复制 B 的 Agent 历史；普通根消息必须显式 @A 才创建 A 的话题 session。

### TC-FTD-04：Codex 精确 fork 协议

操作步骤：运行 `cargo test -p bifrost-admin mock_app_server_forks_at_codex_turn_before_starting_new_turn -- --nocapture`。

预期结果：mock app-server 先收到 `thread/fork {threadId,lastTurnId}`，返回新 thread 后才收到该新 thread 的 `turn/start`。

### TC-FTD-05：Traex 与 Claude Code 能力门禁

操作步骤：运行 `cargo test -p bifrost-admin thread_derivation_capability_matrix_reserves_claude_extension_point -- --nocapture` 和 `cargo test -p bifrost-admin thread_fork_is_precise_for_codex_and_omits_turn_for_traex -- --nocapture`。

预期结果：Traex 支持 completed fork、不宣称 active/turn fork；Traex 请求不发送 `lastTurnId`；Claude Code 返回 unavailable/default capability，保留扩展点。

### TC-FTD-06：飞书话题回复标记

操作步骤：运行 `cargo test -p bifrost-admin topic_reply_request_sets_reply_in_thread -- --nocapture`。

预期结果：话题卡片 reply payload 含 `reply_in_thread=true`；普通 reply 为 false。

### TC-FTD-07：本机锚点自动认领与 transport 降级

操作步骤：运行 `cargo test -p bifrost-admin local_ready_topic_anchor_requires_current_bot_mention -- --nocapture`、`cargo test -p bifrost-admin topic_initialization_is_recovered_from_persisted_binding_after_restart -- --nocapture` 和 `cargo test -p bifrost-admin only_supported_healthy_anchors_are_derivable -- --nocapture`。

预期结果：本机 app-server ready 锚点下的未 mention 回复保持 ambient；只有显式 @ 当前 Bot 后才派生，且不依赖飞书根消息读取；首条已接受事件在进程重启后重投时从 SQLite 恢复同一派生初始化；exec/custom transport、失败锚点和 Traex 无 checkpoint 的 mutable source 均不可派生，降级为普通双消息路径。

### TC-FTD-08：Traex checkpoint 不启动新 turn

操作步骤：运行 `cargo test -p bifrost-admin mock_traex_checkpoint_fork_finishes_without_starting_turn -- --nocapture`。

预期结果：源轮结束后只执行 `thread/fork` 创建不可变 checkpoint，不额外执行 `turn/start`；checkpoint 失败时锚点不可被标成 ready。

### TC-FTD-09：队列、启动恢复与话题失败隔离

操作步骤：运行 `cargo test -p bifrost-admin thread_derivation_anchor_is_consumed_once_for_queued_turns -- --nocapture`、`cargo test -p bifrost-admin thread_anchor_request_planning_covers_active_wait_fallback_and_missing_sources -- --nocapture`、`cargo test -p bifrost-admin traex_checkpoint_requires_app_server_fork_capability -- --nocapture`、`cargo test -p bifrost-admin pending_thread_recovery_is_process_safe_and_released_per_loop -- --nocapture`、`cargo test -p bifrost-admin startup_recovery_replays_persisted_pending_topic_trigger_without_feishu_redelivery -- --nocapture`、`cargo test -p bifrost-admin event_loop_exit_tolerates_topic_recovery_store_failure -- --nocapture`、`cargo test -p bifrost-admin failed_topic_binding_uses_new_message_instead_of_replaying_old_instruction -- --nocapture`、`cargo test -p bifrost-admin thread_progress_card_never_falls_back_to_main_group_send -- --nocapture`、`cargo test -p bifrost-admin topic_terminal_without_progress_card_replies_in_thread_instead_of_main_group -- --nocapture` 和 `cargo test -p bifrost-admin progress_and_terminal_cards_are_both_persisted_as_derivation_anchors -- --nocapture`。

预期结果：派生锚点只消费一次，后续排队轮次续写派生 thread；等待来源结束时若锚点 ready 则精确 fork，若变为 failed、被删除、查询失败、来源提前退出或当前 Runner transport 不支持 fork，则立即恢复冻结的 root + current 双消息上下文；Traex exec 不创建伪 checkpoint；进程启动按当前 provider 原子 claim SQLite nonterminal binding，同一 event-loop 不重复取得记录，同进程替换 loop 在旧 loop 退出前不抢占，旧 loop 退出会释放未完成租约，新进程可接管旧进程租约，并从 binding 内完整事件 JSON 恢复图片/文件，不依赖有界事件历史；failed binding 使用用户的新指令；话题 progress 失败时 terminal 卡仍以 `reply_in_thread=true` 回复原话题，不直接发送到主群；本轮 progress 与 terminal 卡 message_id 均写入可派生锚点。

### TC-FTD-10：话题 Session 配置、命令与多 Bot 后续消息

操作步骤：运行 `cargo test -p bifrost-admin topic_sessions_persist_runner_work_dir_and_resolve_group_metadata -- --nocapture`、`cargo test -p bifrost-admin ordinary_topic_root_requires_mention_before_routing_command_to_group -- --nocapture`、`cargo test -p bifrost-admin bound_topic_does_not_capture_messages_addressed_to_another_bot -- --nocapture`、`cargo test -p bifrost-admin unknown_topic_rejects_ambient_and_other_bot_messages_before_initialization -- --nocapture`、`cargo test -p bifrost-admin external_runner_topic_commands_finalize_the_binding -- --nocapture` 和 `cargo test -p bifrost-admin topic_binding_finalization_tolerates_store_failure -- --nocapture`。

预期结果：新话题继承来源 Runner 和群工作目录，话题内 `/runner`、`/cwd` 持久化到话题自身而不污染 provider 默认值或主群，重复初始化不会覆盖话题已有选择；话题内未 @ 当前 Bot 的消息即使已有 binding 也不执行；普通根消息下显式 @ 当前 Bot 的 `/help` 等 SystemCommand 路由到群主 Session 且不创建话题 binding；明确 @ 其他 Bot 的消息由当前 Bot 拒绝认领。

### TC-FTD-11：fork 响应与滚动进度锚点完整性

操作步骤：运行 `cargo test -p bifrost-admin mock_app_server_rejects_fork_response_without_derived_thread_id -- --nocapture`、`cargo test -p bifrost-admin progress_and_terminal_cards_are_both_persisted_as_derivation_anchors -- --nocapture` 和 `cargo test -p bifrost-admin thread_anchor_request_planning_covers_active_wait_fallback_and_missing_sources -- --nocapture`。

预期结果：`thread/fork` 成功响应缺少新 `thread.id` 时硬失败，绝不退回源 thread；一旦 Codex active thread/turn 元数据就绪，当前及后续滚动生成的 progress card 都写入 `active_ready` 锚点；若用户切换到不支持 fork 的 transport，已持久化 ready 锚点不会被错误 resume。

### TC-FTD-12（回归）：卡片话题命令不派生新 Agent 线程

操作步骤：

1. 运行 `cargo test -p bifrost-admin local_topic_commands_use_source_session_without_claiming_a_thread -- --nocapture`。
2. 运行 `cargo test -p bifrost-admin bound_topic_does_not_capture_messages_addressed_to_another_bot -- --nocapture`。
3. 运行 `cargo test -p bifrost-admin one_message_mentioning_multiple_bots_triggers_each_matching_identity -- --nocapture`。
4. 使用隔离数据目录、fake Feishu OpenAPI 和真实 debug 二进制执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_group_session_context.sh`。
5. 检查 mock Runner prompt、`im_feishu_thread_bindings` 和源 Session 的执行记录。

预期结果：没有 mention、只 @ 某人或只 @ 其他 Bot 时，当前 Bot 不执行、不进入 Guide、不创建 binding；本地卡片锚点下只有显式 `@当前 Bot` 的 `/status`、`/q ... @某人` 等命令才使用锚点 `source_session_key`，当前 Bot mention 从命令中移除，人类/其他 Bot mention 以 `<at id=...>` 保留；普通根话题里的已定向 SystemCommand 使用群主 Session；已有 binding 的话题也必须再次 @ 当前 Bot；同一消息 @ 多个 Bot 时，每个匹配身份都独立分类为 AgentTrigger。已接受的未绑定话题命令不写入 `im_feishu_thread_bindings`、不触发 `thread/fork`，已定向的普通自然语言回复仍按原规则派生。

## 清理步骤

测试脚本退出时终止 fake OpenAPI/Bifrost/Runner 进程并删除临时目录；确认未修改用户数据目录且无测试进程残留。

## 执行记录

- 2026-08-13：PASS — 按 TC-FTD-04 至 TC-FTD-12 的文档命令逐条执行 27 个 focused 测试，全部通过；TC-FTD-01 至 TC-FTD-03 和 TC-FTD-12 的真实链路使用隔离数据目录、双 provider fake Feishu OpenAPI 与最新 `target/debug/bifrost` 执行 `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_group_session_context.sh`，输出 `[feishu-group-session] PASS`。验证未 @、只 @人、只 @其他 Bot 的话题消息均不触发 Agent、不创建 binding；显式 @当前 Bot 的卡片话题命令复用源 Session；同一消息 @多个 Bot 时，各 provider 按自己的 open_id 独立接受。脚本退出后临时进程与数据目录已自动清理。
- 2026-08-12：PASS — 首轮逐条执行 TC-FTD-04 至 TC-FTD-11 所列 21 个 focused 测试全部通过。Review 发现同进程新旧 event-loop 会互抢 `recovering` 记录，改为“进程 owner + loop owner”租约后立即复跑 TC-FTD-09 的 9 个用例全部通过；第二轮 Review 将话题 Runner 初始化改为保留已有话题选择，并立即复跑 TC-FTD-10 的 3 个用例全部通过。覆盖率补强后又执行 TC-FTD-09/10 的话题命令完成态、其他 Bot/ambient 路由、recovery owner 防御性校验，以及 SQLite 写入/退出失败容错用例，全部通过。TC-FTD-01 至 TC-FTD-03 使用隔离数据目录、fake Feishu OpenAPI 和真实 `target/debug/bifrost` 执行 `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_feishu_group_session_context.sh`，输出 `[feishu-group-session] PASS`；脚本已清理临时服务与数据目录。
