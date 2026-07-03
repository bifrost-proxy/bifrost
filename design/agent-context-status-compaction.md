# Agent Context Status 与自动压缩修复

## 背景

Bifrost Agent turn loop 需要在有限的模型 context window 内长期运行：随着 tool result、guide message、pending message、用户新消息不断追加，history 累计 token 会超出阈值，必须触发 compaction 把旧 history 摘要成 summary、保留最近 user carry-over。同时运行中 `/status` 需要展示当前 token 占用、context window 使用、显式压缩次数与最近响应 token，让用户明确是否处于「上下文管理」阶段。

本模块覆盖：

- Turn loop 中的 context compaction（pre-turn / mid-turn / emergency / manual `/compact`）。
- Initial context reinjection（把非 system context 插回 replacement history 的合适位置）。
- Token accounting（`last usage + appended items` 增量口径）。
- Provider context-window overflow 的降级策略。
- 运行中 `/status` 展示与 IM/WebUI 一致性。
- 会话持久化 `compaction` 事件与 replay 边界。
- 飞书 IM progress card 消费 compaction/context 事件的实时刷新。

本轮聚焦几个语义修复：

- **2026-05-08**：把 Bifrost local compaction 与 `~/work/github/codex` 的 Codex local compaction 策略继续对齐——压缩后保留内容顺序、summary 模板、summary 生成请求形态、user message 预算、token snapshot 重算、自动压缩触发时机。
- **2026-05-10**：修复首轮 `/status` 与业务 chat 请求的采样竞态。
- **2026-05-24**：让内置 Bifrost Agent 的 compaction/context progress events 同步刷新飞书 IM progress card。
- **2026-05-28**：收敛 summary 请求失败策略——已知超窗错误先批量裁剪最老 history 再压缩；429/timeout/5xx 按时间梯度最多重试 5 次；仍失败或不可恢复错误直接终止任务。

## 用户目标验证清单

### 必须实现

- 压缩后 replacement history 采用 Codex local compaction 形态：`[recent_user_messages..., summary_message]`。
- User carry-over 使用 token 预算（约 `20_000`），超出的单条 user 使用 middle truncation marker `tokens truncated`。
- Summary message 使用 Codex 原文模板 `{SUMMARY_PREFIX}\n{summary}`。
- Summary 生成请求保持结构化 history（`[system(base_instructions), ...structured_history, user(COMPACTION_PROMPT)]`），不再扁平化成 `[role]: ...` 单条 user。
- Summary 请求超窗时按预算批量丢弃最老 history item；transient error 按 provider `stream_max_retries` 退避，上限 5 次。
- Mid-turn initial context reinjection 不注入 base/system instructions，只重新插入非 system context / memory；插入位置遵循「最后一条 user 之前 → summary 之前 → 追加」优先级。
- Base instructions 不进入 replacement history，只在模型请求构造阶段 prepend；`recompute_token_usage()` 额外算入一次。
- Guide / pending queue continuation 追加消息后先走同一套 mid-turn compaction 检查。
- Token accounting 采用 `last usage + appended items` 增量口径：`effective_token_count() = last_response_tokens + estimate(history[last_response_history_len..])`。
- `compact::should_compact()`、`sessions/all`、`sessions/{key}` detail、`/status.active_status.estimated_context_tokens`、WebUI history HUD 与 IM `/status` 使用同一口径。
- WebUI token/context 展示使用与 IM `/status` 一致的紧凑单位格式（K/M/B，保留一位小数且整数去掉 `.0`）。
- 普通 sampling overflow 不主动删除 live history；只允许 emergency compaction；compaction 失败或仍超窗则显式返回错误。
- `refresh_active_turn_status()` 使用 `effective_token_count()`，history rewrite 后立即刷新。
- Compaction event 记录 `trigger`、`reason`、`phase`、`pre_tokens`、`post_tokens`、`tokens_saved`、`messages_removed`、`compaction_count`、`total_tokens`，emergency 路径额外带 `emergency: true`；不记录 `current_plan`。
- 普通新 turn 采样前恢复 pre-sampling compaction：达到 `model_auto_compact_token_limit` 时使用 `CompactionTrigger::Auto` + `CompactionReason::ContextLimit` + `CompactionPhase::PreTurn` + `InitialContextInjection::DoNotInject`。
- 飞书 IM progress card 消费 `Status`、`ContextUpdated`、`CompactionStarted/Finished/Failed`，随事件更新 token、context usage、`压缩：N 次`。
- Web UI Agent Chat HUD 与 IM `/status` 在 running session 上保持一致，不被旧 JSONL compaction 快照覆盖。

### 必须不破坏

- JSONL 历史文件格式不变；旧 session 打开仍能 replay。
- `update_plan` runtime 语义不变；compaction event 不写入、恢复、清空 plan。
- 手动 `/compact` 命令继续可用，语义与自动 compaction 对齐。
- 请求级历史窗口裁剪配置若历史存在，被 serde/JSON patch 自然忽略，不再作为产品语义。
- 现有 provider transient retry（`is_retryable_error()`）语义在普通模型请求路径继续生效。

### 必须真实验证

- 26 项 Rust 单测覆盖 replacement history 结构、initial context reinjection、token accounting、guide/pending continuation、compaction event、Codex summary request shape、context-window retry、transient retry、token snapshot、pre-sampling compaction、progress card。
- Shell E2E 覆盖运行中 `/status` 竞态与 guide/pending queue。
- human_tests 更新覆盖新语义，编号 TC-BC-21 ~ TC-BC-37；新增 `human_tests/agent-im-card-compression.md`。

## 现状核查与根因

相关代码路径：

- `crates/agent/src/compact.rs`
  - `compact_session()` 构造 replacement history。
  - `collect_user_messages()` 选择压缩后保留的近期用户消息（旧名 `collect_recent_user_messages` 已重命名）。
  - `insert_initial_context_before_last_user_message()` 控制 mid-turn reinjection 位置。
- `crates/agent/src/session.rs`
  - `AgentSession::track_token_usage()` / `effective_token_count()` 维护压缩触发口径。
  - `AgentSession::build_messages()` 在请求构造阶段 prepend prompt prefix / memory，并对 history 中已经存在的等价非 system context 做去重。
- `crates/agent/src/session/turn_loop.rs`
  - `run_turn_with_mcp_multimodal()` 控制 mid-turn、guide / pending continuation、overflow retry（原 `session.rs` 拆分到 session 子模块的 `turn_loop`）。
  - `compact_mid_turn_if_needed()` 复用同一套 mid-turn compaction 边界。
  - `record_compaction_event()` 写入持久化统计。
  - `build_mid_turn_initial_context()` 计算 mid-turn 注入的非 system initial context。
- `crates/agent/src/session_status.rs`
  - `refresh_active_turn_status()` 将 session state 投影到运行中 `/status`。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`
  - `ImAgentProgressSnapshot::apply_event()` 消费 Agent progress events 并渲染飞书 progress card。
  - `format_status_panel_title()` / `format_footer_markdown()` 展示 token、context 和压缩次数。
- `crates/agent/src/persistence.rs`
  - `ConversationRecorder::record_compaction()` 持久化 compaction 事件。

审计确认的根因：

1. Bifrost replacement history 仍是 `[summary, recent...]`，而 local compaction 应该是 `[recent user messages..., summary]`。
2. Bifrost mid-turn compaction 只注入轻量 reminder，仍依赖 `build_messages()` 每次 prepend prompt prefix；应该把 Codex local history 中的非 system initial context 重新插入 replacement history 边界。
3. Guide / pending queue continuation 曾在追加消息后直接进入下一 loop，绕过部分 mid-turn compaction 检查。
4. Token accounting 只保存 `last_response_tokens` 或退回 full-history estimate，不能表达「last API usage + last model item 之后新增 items 估算」。
5. Bifrost 曾在普通 sampling overflow 后保留 emergency compact + trim retry；这不是 Codex 普通 sampling overflow 语义，需要删除静默 trim live history 的 fallback。
6. history rewrite 后 active `/status` 曾存在短窗口旧快照，`compaction_count`、`history_version`、context token 口径可能与实际 session 不一致。
7. 后续复核发现 Bifrost 仍保留若干非 Codex local compaction 细节：summary prompt/prefix 不是 Codex 原文；summary 生成请求把完整 history 扁平化为 `[role]: ...` 单条 user message，且把 compaction prompt 放在 system message；user carry-over 用 char/byte 预算而非 token 预算；replacement history 直接 clone `ChatMessage`，可能保留 `content_parts`；压缩后 token snapshot 没有把 base instructions 算入；普通新 turn 采样前缺少 Codex 当前仍保留的 pre-sampling compaction。
8. 飞书 progress card 的 snapshot 只有 `status: Option<ActiveTurnStatus>`，但内置 Agent compaction 成功时先发出 `CompactionFinished { progress.context.compaction_count = N }`，未必紧跟新的 `Status` 事件。旧实现把 `ContextUpdated` 与 compaction 事件直接丢弃，导致卡片增量更新链路没有新的 status hash，用户在 IM 卡片上看到的「压缩：N 次」可能落后于真实 session。

## 产品语义

- **Compaction 是显式历史改写**：只有 summary 生成、replacement history 安装、event 落盘才是 compaction；请求级消息数量裁剪不是 compaction，不进入 `compaction_count`。
- **Token 口径统一**：所有展示与触发使用同一个 `effective_token_count()`；不允许 sessions/all detail、WebUI HUD、IM `/status` 出现不同口径。
- **Overflow 是显式错误**：普通 sampling overflow 不再静默 trim live history；用户看到明确失败原因，可手动 `/compact` 或缩短输入。
- **Plan 与 compaction 解耦**：compaction event 不影响 plan runtime；plan 的运行态恢复只接受 `plan_updated` / `plan_cleared`。

## 技术细节

### 1. Replacement history 改为 summary-last

`compact_session()` 现在按 Codex local compaction 构造：

```text
[...recent_user_messages, summary_message]
```

`collect_user_messages()` 只返回真实 user message 的纯文本内容，过滤历史 compaction summary 与 Bifrost prompt-prefix 产生的 contextual user sections。`build_compacted_history()` 再用这些字符串重建 text-only `ChatMessage::user`，因此不会把 assistant/tool/tool_result 原样搬进 replacement history，也不会把多模态 `content_parts` 或图片内容继续留在压缩后的 carry-over 中。

User message carry-over 预算使用 Codex 相同的 `20_000` approximate tokens。预算从最新 user message 向前选择；遇到超出预算的单条 user message 时，使用 Codex 风格的 middle truncation marker（`tokens truncated`）截断后保留。

Summary message 使用 Codex 原文模板：

```text
{SUMMARY_PREFIX}\n{summary}
```

`is_summary_message()` 按 Codex 的 `SUMMARY_PREFIX + "\n"` 边界识别，避免普通用户消息仅以前缀文本开头时被误判。

### 2. Mid-turn initial context reinjection 不包含 base instructions

`build_mid_turn_initial_context()` 不再只创建轻量 reminder。mid-turn compaction 接收当前 turn 已解析的 `prompt_prefix` 与 optional `memory_message`，但只把非 system 的 concrete initial context items 插入 compacted history。

Base/system instructions 对齐 Codex 的 `Prompt.base_instructions` 语义：它们不作为 history item 进入 replacement history，只在模型请求构造阶段 prepend，并在 token snapshot 重算时额外计入一次。

插入位置遵循：

1. 优先插入到最后一个真实用户消息之前。
2. 如果只剩 summary，则插入到 summary 之前。
3. 如果没有 user-like item，则追加。

Bifrost 仍保留自己的 prompt-prefix 架构：常规 `build_messages()` 会继续 prepend prompt prefix，但会跳过 replacement history 中已经存在的等价非 system context / memory message，避免 mid-turn compaction 后下一次请求重复看到 developer、contextual user 或 memory。system/base message 永远不通过 history 去重。

### 2.1 Summary 生成请求保持结构化 history

`compact_session()` 生成 handoff summary 时不再把 session history 格式化成 `[role]: ...` 文本，也不再把 `COMPACTION_PROMPT` 作为 system message 发送。请求构造改为 Codex local compaction 形态：

```text
[system(base_instructions if any), ...structured_history, user(COMPACTION_PROMPT)]
```

保留 assistant tool calls、tool results、多模态 user message 等原始结构化消息形态，让 summary 模型看到的输入边界与正常模型请求一致。base instructions 只作为本次请求的 system item 参与，不写入 compacted replacement history。

当 summary 生成请求自身遇到 context-window overflow 时，`compact_session()` 会对该请求副本执行 Codex-style retry：保留开头的 base/system instructions 和末尾的 `COMPACTION_PROMPT`，只移除 structured history 中最老的 item 后重试。这个裁剪只作用于 summary 请求副本，不修改 `session.history`；最终 replacement history 仍从真实 session history 收集 user messages。

Non-context-window 的 transient error（429、5xx、timeout、connection reset）按 provider 合并后的 `stream_max_retries` 预算做 exponential backoff retry。context-window retry 仍优先走移除最老 history item 的路径，并在裁剪后重置 transient retry 计数。

2026-05-28 起，summary 生成请求增加两层失败保护：

1. 发请求前根据 `model_context_window` 计算 safe request budget（默认 70% 并预留 completion headroom），如果 structured history 已超过预算，先批量丢弃最旧 history item，避免直接构造超窗请求。
2. 如果 provider 仍返回 `context_length_exceeded` / context window / token limit，按当前请求 token 数继续批量降级，而不是每次只删一条历史导致请求风暴。

Transient / 未知可恢复错误的重试预算统一限制为最多 5 次；provider 配置更高时向下夹到 5。重试全部失败后 `compact_session_with_hooks()` 返回错误，mid-turn / guide / pending queue 压缩调用把错误向外传播并终止当前 turn，不再只 warn 后继续 loop。普通模型请求路径已有同类保护：`is_retryable_error()` 覆盖 429、rate limit、timeout、connection、overloaded、5xx 等，turn loop 最多 5 次指数退避。

### 3. Token accounting 改为 last usage + appended items

`AgentSession` 新增 `last_response_history_len`：

- 普通模型响应写入 `last_response_tokens`，并记录当时的 history 边界。
- Assistant 文本 / tool_calls 写入 history 后，如果它正是刚返回的模型 item，则边界前移到该模型 item 之后。
- 后续 tool result、guide message、pending message、新用户消息不清空 `last_response_tokens`，而是作为「last model item 之后新增 items」参与估算。
- Compaction、rollback、clear、trim 等 history rewrite 会清空 `last_response_tokens` 与 `last_response_history_len`。

`effective_token_count()` 现在返回：

```text
last_response_tokens + estimate(history[last_response_history_len..])
```

当没有有效 API snapshot 时才退回 `estimate_tokens()`。`compact::should_compact()`、session list/detail 的 `estimated_tokens`、运行中 `/status.active_status.estimated_context_tokens` 使用同一口径。运行中的 session 以 `ActiveTurnStatus` 为 token/context 权威来源：`sessions/all`、`sessions/{key}` detail、WebUI history timeline polling 和 IM `/status` 看到的累计 token、最近 context token、context window 与占比必须一致；JSONL history 只作为消息 timeline 与持久化快照来源，不允许用旧 `assistant_message` / `compaction` 事件覆盖 running active status。WebUI token/context 展示使用与 IM `/status` 相同的紧凑单位格式。

### 4. Guide / pending continuation 共用 mid-turn compaction 边界

抽取 `compact_mid_turn_if_needed()`，并在以下继续 loop 路径复用：

- Tool result 与 mid-turn guide message 处理完成后。
- Turn-end guide message 注入后。
- `pending_messages` 弹出并追加到 history 后。

新增 queued/guide input 后，如果下一步仍要进入模型请求，会先走同一套 `should_compact()` 与 `record_compaction_event()`，避免「刚追加大消息就直接进入下一次请求」的抖动。

### 5. Emergency overflow 对齐 Codex，不静默裁剪 live history

普通 sampling overflow 不主动删除 live history 后重试。Bifrost 只允许先尝试 emergency compaction；如果 compaction 失败或 compaction 后 provider 仍报告 context-window overflow，则标记当前 context window 已满、刷新 active status，并把错误显式返回给调用方。

本轮对该特殊行为加边界：

- Emergency compaction 使用与 mid-turn compaction 相同的 summary-last history 和非 system initial context reinjection。
- Compaction 成功后立即刷新 active status。
- Emergency compaction 写入统一 `compaction` 事件，并带 `emergency: true`。
- 不再执行 `trim_oldest_messages_count()` 这类 live history 删除 fallback。

### 6. Status 与 persistence 同步

`refresh_active_turn_status()` 使用 `session.effective_token_count()`，与 compaction trigger 口径一致。所有 compaction 路径通过 `record_compaction_event()` 记录：

- `trigger`、`reason`、`phase`
- `pre_tokens`、`post_tokens`、`tokens_saved`、`messages_removed`
- `compaction_count`
- `total_tokens`
- Emergency 路径额外包含 `emergency: true`

Compaction event 不记录 `current_plan`，persistence 回放时也不从 compaction metadata 恢复、覆盖或清空 plan。Plan 的运行态恢复只接受 `plan_updated` / `plan_cleared`，保持与 Codex Memento/handoff-summary 方案一致：summary 是历史重写，不是 runtime checkpoint。

Mid-turn compaction 后，同一 turn 的下一次模型请求会额外收到 turn-local plan runtime context，用于恢复 `session.current_plan` 的当前 snapshot。该提示不是 compaction event，不写入 replacement history，不进入 JSONL，也不跨普通新 user turn 重放；它只修复 summary replacement history 丢失模型可见 plan 状态的问题。

History rewrite 后同步刷新 active status，避免 `/status` 展示旧 `history_version` 或旧 context token。

### 7. 压缩后 token snapshot 纳入 base instructions

Codex 的 `recompute_token_usage()` 会在 replacement history 上额外估算 base instructions。Bifrost 的 Chat Completions 架构虽然把 prompt prefix 作为消息 prepend，但压缩后 `last_response_tokens` 也必须包含 base/system instructions，否则 `/status` 和下一轮 `should_compact()` 会低估 context。

`compact_session()` 现在接收当前 turn 的 base instructions，并在安装 replacement history 后调用：

```text
estimate(history) + estimate(system(base_instructions))
```

手动 `/compact` 使用当前配置解析出的 base instructions；mid-turn / emergency compaction 使用本 turn 已解析的 base instructions。

### 8. 自动压缩触发时机对齐 Codex pre-sampling 与 post-sampling follow-up

Bifrost 在普通 turn 采样前恢复 Codex 当前的 pre-sampling compaction：解析 base instructions 与 prompt prefix 后、把本轮新 user message 写入 history 之前，如果上一轮累计 token usage 已达到 `model_auto_compact_token_limit`（解析后的有效阈值），执行：

```text
CompactionTrigger::Auto
CompactionReason::ContextLimit
CompactionPhase::PreTurn
InitialContextInjection::DoNotInject
```

该路径不注入 initial context，压缩后仍由正常 `build_messages()` 在随后模型请求里 prepend prompt prefix。这样新 user message 不会被卷入 pre-sampling compaction summary，base instructions 也不会进入 replacement history。

自动压缩仍保留在会继续进入下一次模型请求的路径上：

- Tool result / guide message 处理后继续 loop。
- Turn-end guide 或 pending queue 追加后继续 loop。
- Context overflow emergency fallback。

Post-sampling 路径仍使用 mid-turn `BeforeLastUserMessage` 注入非 system initial context，与 Codex `token_limit_reached && needs_follow_up` 语义保持一致。

### 9. 飞书 IM progress card 消费 compaction/context 事件

`ImAgentProgressSnapshot` 新增 `context: Option<AgentContextSnapshot>`，在 `apply_event()` 中统一处理：

- `Status`：保存完整 `ActiveTurnStatus`，同步生成 context snapshot。
- `ContextUpdated`：更新 context；如果已有 status，则把 estimated context、window、usage percent、compaction count、history version、message count、user turn count、last/total tokens 回写到 status。
- `CompactionStarted` / `CompactionFinished` / `CompactionFailed`：从 progress.context 走同一套 context 同步逻辑。
- WebUI history 视图：消息列表继续从 JSONL events 恢复；若同 session 在 `sessions/all` 或 detail 中仍是 running，则 telemetry 最终再覆盖 active status，避免旧 compaction 快照让顶部 Token/Context HUD 与 M 通道 `/status` 不一致。

飞书卡片仍优先展示 status 中的运行态字段（loop、work_dir、队列、guide 等）；当 status 暂不可用时，状态面板标题与 footer 会回退到 context snapshot，至少保证 token、context usage 与 `压缩：N 次` 能随 compaction event 更新。这样不会改变最终回复卡片、工具面板与计划面板结构，只修正状态区数据源。

## 依赖项

- `crates/agent/src/session.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/compact.rs`
- `crates/agent/src/compaction_hooks.rs`
- `crates/agent/src/session_status.rs`
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`
- `crates/agent/src/persistence.rs`
- `crates/bifrost-admin/src/state.rs`（测试隔离修正）
- `e2e-tests/tests/test_agent_builtin_status_runtime.sh`
- `e2e-tests/tests/test_im_guide_queue_human_api.sh`
- `human_tests/agent-builtin-commands.md`
- `human_tests/agent-im-card-compression.md`
- `human_tests/readme.md`

## `/status` 显式压缩与上下文管理展示（2026-05-28）

`compaction_count` 只表示显式 summary/manual/emergency compaction。Bifrost Agent Loop 不提供请求级消息数量硬裁剪；历史上下文默认完整进入 `build_messages()`，然后统一通过 history sanitize 保证 tool call / tool result 配对合法。

上下文大小控制对齐 Codex-style 路径：

1. 常规请求依赖 provider context window、`model_auto_compact_token_limit`、pre-turn / mid-turn / manual compaction、summary replacement history 与 token snapshot。
2. `/status` 与 WebUI token HUD 展示 `Context 用量`、`显式压缩次数`、`上下文管理`；`上下文管理` 表达「按 token/context budget 与 compaction 管理」，不再暗示默认存在消息数量窗口。
3. Provider context-window overflow 只允许触发有记录的 emergency compaction；若仍超限或 compaction 失败，则保留 live history 并显式返回错误，不再批量删除最老消息。
4. 旧的请求级数量窗口配置、状态字段、Web 展示与 patch 入口已删除；未知旧配置由 serde/JSON patch 自然忽略。

## 实现切分

### Phase 1：Replacement history 与 token accounting

- `compact_session()` 采用 summary-last。
- `collect_user_messages()` 使用 token 预算与 truncation marker。
- `AgentSession` 引入 `last_response_history_len` 与增量口径。
- 单测覆盖 replacement history 结构、token 增量、text-only rebuild。

### Phase 2：Mid-turn / Emergency / Pre-sampling compaction

- `build_mid_turn_initial_context()` 排除 base/system。
- `compact_mid_turn_if_needed()` 在 tool result、guide、pending queue 处复用。
- 普通 sampling overflow 移除 live history trim fallback。
- 新增 pre-sampling `DoNotInject + PreTurn` 路径。

### Phase 3：Summary request shape 与 retry

- 结构化 history + `COMPACTION_PROMPT` as user。
- Safe request budget 与批量丢弃。
- Transient 5xx / timeout / connection reset 按 provider 预算最多 5 次退避。
- 失败向外传播，终止当前 turn。

### Phase 4：Status / Progress card / WebUI HUD 同步

- `refresh_active_turn_status()` 用 `effective_token_count()`。
- History rewrite 后立即刷新。
- `ImAgentProgressSnapshot` 消费 `ContextUpdated` / Compaction 事件。
- WebUI HUD 使用与 IM `/status` 相同格式化。

### Phase 5：human_tests 与文档

- 新增 TC-BC-21 ~ TC-BC-37 用例，更新 `agent-builtin-commands.md`。
- 新增 `agent-im-card-compression.md` 覆盖飞书 IM progress card 实时刷新。
- 更新 `human_tests/readme.md` 索引与总数。

## 测试方案

### 单元测试

1. `compact::tests::test_build_compacted_history_places_summary_after_recent_messages`：replacement history 是 recent messages 在前、summary 在末尾。
2. `compact::tests::test_insert_initial_context_before_last_real_user_message`：initial context 插入最后一个真实 user 前，summary 仍在末尾。
3. `compact::tests::test_should_compact_adds_items_after_last_model_snapshot`：压缩触发使用 last usage + appended items。
4. `session::tests::test_history_growth_after_response_is_incrementally_accounted`：新增用户消息不清空 snapshot，而是增量计入 context。
5. `session::tests::test_model_message_advances_incremental_token_boundary`：assistant model item 推进 last model boundary。
6. `session::tests::test_queued_continuation_compacts_before_next_model_request`：pending continuation 追加大消息后先 compact，reinjected context 包含非 system prompt context / memory，不含 base instructions。
7. `session::tests::test_context_rewrite_refreshes_active_status_snapshot`：history rewrite 后 active status 与 session 同步。
8. `session::tests::test_record_compaction_event_includes_emergency_and_total_tokens`：emergency compaction 事件字段完整，且不含 `current_plan`。
9. `session_status::tests::test_active_turn_status_text_contains_runtime_metrics`：`/status` 文案字段。
10. `compact::tests::test_codex_compaction_templates_are_exact`：compaction prompt 与 summary prefix 与 Codex 模板逐字一致。
11. `compact::tests::test_build_compaction_messages_uses_codex_local_request_shape`：summary 生成请求保留 structured history，`COMPACTION_PROMPT` 作为最后一条 user message，不再 `[role]: ...` 扁平化。
12. `compact::tests::test_compaction_retries_context_window_error_by_dropping_history_batch`：summary 请求超窗后按预算批量丢弃请求副本中的最老 history item 并重试，保留 base/system 与 compact prompt。
13. `compact::tests::test_compaction_retries_transient_error_using_provider_budget`：summary 请求遇到 transient 500 时按 provider `stream_max_retries` 预算重试且不裁剪 history。
14. `compact::tests::test_collect_user_messages_caps_preserved_user_budget`：user carry-over 使用 token 预算并产生 `tokens truncated` marker。
15. `compact::tests::test_build_compacted_history_rebuilds_text_only_user_messages`：多模态 user message 压缩后重建为 text-only user message。
16. `session::tests::test_recompute_token_snapshot_includes_base_instructions`：压缩后 token snapshot 包含 base instructions。
17. `persistence::tests::test_compaction_does_not_mutate_plan_runtime_state`：compaction event 不作为 plan runtime source。
18. `session::tests::test_pre_sampling_auto_compacts_before_model_request`：普通 turn 采样前如果历史已超过阈值，用 `DoNotInject` 先压缩旧 history，再追加本轮 user message 并请求模型。
19. `session::tests::test_mid_turn_initial_context_excludes_base_instructions`：mid-turn initial context builder 不把 base/system instructions 放进 replacement history。
20. `session::tests::test_build_messages_dedupes_injected_mid_turn_context`：本次请求选中的 history 已携带非 system context / memory 时不重复 prepend，system/base 仍只出现一次。
21. `session::tests::test_build_messages_dedupes_context_against_full_history`：build_messages 使用完整 history 做非 system context 去重，不再因请求级数量窗口产生遗漏。
22. `progress_card::tests::progress_snapshot_updates_status_from_compaction_context`：已有 status 时，`CompactionFinished` 的 context 把 `compaction_count`、history version、token / context 字段回写到飞书卡片状态。
23. `progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available`：尚无 status 时，`ContextUpdated` 也能让飞书卡片状态区展示 token、context 与压缩次数。

### E2E 测试

更新并执行 `e2e-tests/tests/test_agent_builtin_status_runtime.sh`：

1. mock provider 返回较小 `usage.total_tokens = 17`。
2. 工具调用产生大体积输出并进入第二轮模型请求。
3. 运行中 `/status` 断言：
   - 首轮长模型请求启动后，脚本先等待 mock 记录到第一轮模型请求，再轮询到真实 `active_status` 做断言。
   - 如果 `/status` 在业务请求之前误创建了空闲 session，后续带 `work_dir` 的 turn 必须覆盖该 session 的工作路径并重置上下文。
   - `active_status.current_loop_iteration == 2`
   - 压缩前路径验证 `active_status.last_response_tokens == 17` 且 `active_status.estimated_context_tokens > 10017`
   - 压缩后路径验证 `active_status.compaction_count == 1`
   - 文本 `Context 用量` 与 JSON 字段一致，并使用 K/M/B 格式化窗口值（例如 `250K`）
   - 文本最近响应显示与 JSON 字段一致的格式化值
4. CI 高负载下先等待 mock 记录到压缩后的第二轮模型请求，再进入 `/status` 轮询；第二轮模型请求保留足够长的 mock 延迟窗口，并在轮询期间保存最后一次 `active_status` 响应。

由于本轮触达 guide / pending continuation，补跑 `e2e-tests/tests/test_im_guide_queue_human_api.sh`，验证 turn-end guide、FIFO queue、guide 优先级与空白忽略仍符合预期。

2026-05-24 新增覆盖：执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture` 和 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture`，作为飞书 progress card payload 层的可自动执行 E2E 替代验证；如需真实飞书发送链路，可在已配置 Feishu provider 的默认数据目录下触发一次会自动压缩的内置 Agent 会话，并观察卡片状态区。

### 真实场景测试 human_tests

更新 `human_tests/agent-builtin-commands.md`：

1. 新增 `TC-BC-21`：运行中 `/status` 在工具结果追加后展示 last API usage + 新增 items 的 context 口径。
2. 新增 `TC-BC-22`：自动压缩判断使用增量 token accounting，不被旧 snapshot 遮蔽。
3. 新增 `TC-BC-23`：emergency compaction 记录完整统计事件。
4. 新增 `TC-BC-24`：guide / pending queue 追加消息后继续 loop 前先执行 mid-turn compaction。
5. 更新 `TC-BC-25`：emergency compaction 改写 history 后立即刷新运行中 `/status`；provider overflow 不再静默 trim live history。
6. 新增 `TC-BC-27`：Codex local compaction 模板、纯文本 replacement history 与 token budget 对齐。
7. 新增 `TC-BC-28`：压缩后 token snapshot 包含 base instructions。
8. 新增 `TC-BC-29`：mid-turn compact 后 replacement history 不包含 base instructions，build_messages 不重复注入非 system context / memory，token snapshot 只额外计入一次 base instructions。
9. 新增 `TC-BC-30`：普通 turn pre-sampling auto compaction 对齐 Codex 当前 `DoNotInject + PreTurn` 路径。
10. 新增 `TC-BC-31`：summary 生成请求使用 Codex local structured history 形态，把 `COMPACTION_PROMPT` 作为最后一条 user message。
11. 新增 `TC-BC-32`：summary 请求超上下文后按预算批量丢弃请求副本中的最老 history item 并重试。
12. 新增 `TC-BC-33`：summary 请求 transient error 按 provider retry budget 退避重试。
13. 更新 `TC-BC-35`：`/status` 区分显式压缩次数与 token/context budget 驱动的上下文管理，不再报告请求级数量裁剪。
14. 更新 `TC-BC-36`：构造超过 50 条短历史，验证最终模型请求仍包含最早和最新探针消息，证明常规请求使用完整 history。
15. 新增 `TC-BC-37`：running history 深链下 WebUI Token/Context HUD 与 `sessions/all` / `/status.active_status` 同步，不再使用旧 JSONL compaction 快照。

同步更新 `human_tests/readme.md` 中 Agent 内置命令测试用例数量。

新增 `human_tests/agent-im-card-compression.md`：覆盖飞书 IM progress card 从 `Status`、`ContextUpdated` 与 `CompactionFinished` 事件同步压缩次数的真实场景验证，并记录本轮实际执行的 Rust payload 层命令结果。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

## 校验要求

按顺序执行：

1. `cargo test -p bifrost-agent compact::tests:: -- --nocapture`
2. `cargo test -p bifrost-agent session::tests:: -- --nocapture`
3. `cargo test -p bifrost-agent session_status::tests:: -- --nocapture`
4. `bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`
5. `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`
6. `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture`
7. `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture`
8. 按 `human_tests/agent-builtin-commands.md` 与 `human_tests/agent-im-card-compression.md` 新增/受影响用例逐条执行并记录结果
9. `cargo fmt --all -- --check`
10. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
11. `cargo test --workspace --all-features`
12. 按修改范围执行 `bash scripts/ci/local-ci.sh --skip-e2e`，用上述 targeted E2E 覆盖受影响链路。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：replacement history 结构、token accounting 增量口径、pre-sampling / mid-turn / emergency compaction、summary request retry、`/status` 与 IM/WebUI 一致、progress card 消费 context 事件。
- 复核 diff：`compact.rs`、`session.rs`、`session/turn_loop.rs`、`session_status.rs`、`progress_card.rs`、`persistence.rs`、E2E 脚本、human_tests、readme。
- 重点 review：guide/pending queue 未绕过 mid-turn compaction；base instructions 未泄漏进 replacement history；request budget 与批量丢弃逻辑正确；transient retry 上限被夹到 5；plan runtime 未被 compaction 覆盖；飞书 progress card 状态区在无 status 时能回退。
- 复测：targeted Rust 单测、shell E2E、progress card payload 单测、human_tests 用例。

### 第 2 轮

- 复查第 1 轮修复后的 diff、readme 索引、design 字段同步。
- 重点 review：CI 高负载下 `/status` 轮询稳定；WebUI HUD 与 `/status` 数值一致；`/compact` 手动路径与自动路径一致；旧请求级数量窗口配置遗留是否已从 patch/status/UI 完全清理。
- 复跑受影响测试，如失败必须重跑并修复。

## 本轮验证结果（2026-05-10）

GitHub Actions `E2E Shell (Linux, shard 1/3)` 暴露首轮 `/status` 采样竞态：固定 `sleep 0.8` 可能先于业务 chat 请求创建空闲 session，导致后续 `work_dir` 丢失并显示 `N/A`。本轮修复：

1. `AgentSessionManager::{take_session_with_work_dir,try_take_session_with_work_dir}` 对已存在 session 也应用显式 `work_dir` override。
2. `POST /_bifrost/api/im-gateway/agent/chat` 的 `/status` 改为纯读路径。
3. `e2e-tests/tests/test_agent_builtin_status_runtime.sh` 首轮 `/status` 改为轮询真实 `active_status.session_key/work_dir/current_loop_iteration`。
4. 单元回归覆盖 `/status` 先创建 session、随后业务 turn 带 `work_dir` 的场景。

## 本轮验证结果（2026-05-08）

已执行并通过：

1. `cargo test -p bifrost-agent compact::tests:: -- --nocapture`：20 passed
2. `cargo test -p bifrost-agent session::tests:: -- --nocapture`：68 passed
3. `cargo test -p bifrost-agent session_status::tests:: -- --nocapture`：2 passed
4. `bash e2e-tests/tests/test_agent_builtin_status_runtime.sh`：`[agent-builtin-status-runtime] PASS`
5. `bash e2e-tests/tests/test_im_guide_queue_human_api.sh`：`[im-guide-queue-human-api] PASS`
6. human_tests 用例 TC-BC-21、TC-BC-22、TC-BC-24、TC-BC-25、TC-BC-26、TC-BC-27、TC-BC-28、TC-BC-29、TC-BC-30、TC-BC-31、TC-BC-32、TC-BC-33 全部通过
7. `cargo fmt --all -- --check`：通过
8. `cargo clippy -p bifrost-agent --all-targets --all-features -- -D warnings`：通过
9. `cargo test -p bifrost-agent --all-features`：lib 566 passed，P1 tools E2E 7 passed，session skills integration 6 passed，doc-test 1 passed
10. `cargo test --workspace --all-features`：仍被既有 traffic/port 脏改阻塞（`crates/bifrost-e2e/src/tests/traffic_cli.rs:55` 缺 `listener_port`），不在本模块修改范围内
11. `cargo clippy --workspace --all-targets --all-features -- -D warnings`：同样被上项阻塞
12. `cargo test -p bifrost-admin state::tests::group_name_cache_ -- --nocapture`：4 passed

## 风险与决策点

- Provider `stream_max_retries` 上限 5 是否过于保守：当前策略保证不会因为 provider 长时间波动导致压缩风暴，代价是极端情况下需要用户手动重试。
- Summary safe budget 系数 70% 是启发式：如果模型 provider 报出 context window 与实际差异较大，可能仍触发降级；后续可根据实际观测调整。
- Pre-sampling compaction 与 mid-turn 的边界：pre-sampling 用 `DoNotInject`，mid-turn 用 `BeforeLastUserMessage`；若未来引入更多 turn 阶段，需要显式覆盖，避免误用。
- 飞书 IM progress card 回退到 context snapshot 时展示口径与 `/status` 略简：若产品希望完全一致，需要把 context snapshot 扩展为 mini status。
- 请求级数量窗口是历史遗留；只要模型 provider 生态出现新的 truncation 需求，需要重新设计而不是复用旧字段。

## 文档更新要求

- 更新 `design/agent-context-status-compaction.md`
- 更新 `human_tests/agent-builtin-commands.md`
- 更新 `human_tests/agent-im-card-compression.md`
- 更新 `human_tests/readme.md`
