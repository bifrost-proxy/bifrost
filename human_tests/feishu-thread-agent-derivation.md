# 飞书群话题 Agent 派生真实场景测试

## 功能模块说明

验证飞书群聊话题从卡片/普通消息派生独立 Agent Session，覆盖本地 checkpoint、跨设备等价降级、多 Bot 隔离、严格双消息上下文、话题内回复与 Runner 能力边界。

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

操作步骤：运行 `cargo test -p bifrost-admin local_ready_topic_anchor_is_claimed_without_mention_or_root_fetch -- --nocapture`、`cargo test -p bifrost-admin topic_initialization_is_recovered_from_persisted_binding_after_restart -- --nocapture` 和 `cargo test -p bifrost-admin only_supported_healthy_anchors_are_derivable -- --nocapture`。

预期结果：本机 app-server ready 锚点下的首条话题消息即使未 @ 当前 Bot、且飞书根消息读取不可用，也直接派生；首条事件在进程重启后重投时从 SQLite 恢复同一派生初始化；exec/custom transport、失败锚点和 Traex 无 checkpoint 的 mutable source 均不可派生，降级为普通双消息路径。

### TC-FTD-08：Traex checkpoint 不启动新 turn

操作步骤：运行 `cargo test -p bifrost-admin mock_traex_checkpoint_fork_finishes_without_starting_turn -- --nocapture`。

预期结果：源轮结束后只执行 `thread/fork` 创建不可变 checkpoint，不额外执行 `turn/start`；checkpoint 失败时锚点不可被标成 ready。

### TC-FTD-09：队列、启动恢复与话题失败隔离

操作步骤：运行 `cargo test -p bifrost-admin thread_derivation_anchor_is_consumed_once_for_queued_turns -- --nocapture`、`cargo test -p bifrost-admin traex_checkpoint_requires_app_server_fork_capability -- --nocapture`、`cargo test -p bifrost-admin pending_thread_recovery_is_atomically_claimed_once_per_process -- --nocapture`、`cargo test -p bifrost-admin startup_recovery_replays_persisted_pending_topic_trigger_without_feishu_redelivery -- --nocapture`、`cargo test -p bifrost-admin failed_topic_binding_uses_new_message_instead_of_replaying_old_instruction -- --nocapture`、`cargo test -p bifrost-admin thread_progress_card_never_falls_back_to_main_group_send -- --nocapture`、`cargo test -p bifrost-admin topic_terminal_without_progress_card_replies_in_thread_instead_of_main_group -- --nocapture` 和 `cargo test -p bifrost-admin progress_and_terminal_cards_are_both_persisted_as_derivation_anchors -- --nocapture`。

预期结果：派生锚点只消费一次，后续排队轮次续写派生 thread；Traex exec 不创建伪 checkpoint；进程启动按当前 provider 原子 claim SQLite nonterminal binding，同进程多个 event-loop 只有一个取得记录，并从 binding 内完整事件 JSON 恢复图片/文件，不依赖有界事件历史；failed binding 使用用户的新指令；话题 progress 失败时 terminal 卡仍以 `reply_in_thread=true` 回复原话题，不直接发送到主群；本轮 progress 与 terminal 卡 message_id 均写入可派生锚点。

## 清理步骤

测试脚本退出时终止 fake OpenAPI/Bifrost/Runner 进程并删除临时目录；确认未修改用户数据目录且无测试进程残留。
