# Agent Plan Lifecycle

## 背景

Bifrost 内置 Agent 提供 `update_plan` 工具让模型主动维护当前任务的 checklist；结果会显示在 Web/Admin 顶部 plan 面板、飞书 IM 进度卡片以及 `/status` 输出中。长任务、Codex-parity 场景下模型会多次调用 `update_plan` 提交完整快照，用户期待每次卡片看到的是“当下的短清单”而不是历史累计。

旧实现 `crates/agent/src/session/turn_loop.rs::reconcile_plan_update()` 强制保留同名 completed 步骤，并且在每轮工具批结束后从 `tool_calls_log.iter().rfind(update_plan)` 反解析最近一次 `UPDATE_PLAN:{json}` 字符串信号，导致同一次成功调用被后续无关 shell/read 工具反复消费；配合 completed 继承语义，卡片上的 plan 会从 8~10 步单调增长到 20~50 步，严重偏离用户当前目标，甚至在 `/compact` 之后模型会重新拆细一次覆盖上限。

本方案参考 `~/work/github/codex` 的 `codex-rs/core/src/tools/handlers/plan.rs`、`codex-rs/protocol/src/plan_tool.rs`、`codex-rs/app-server/src/bespoke_event_handling.rs` 与 `codex-rs/exec/src/event_processor_with_jsonl_output.rs`：让 `update_plan` 成为“权威当前快照”，历史由 session JSONL 事件保存，runtime 只维护 `session.current_plan` 与 `PlanUpdated` 展示事件。

## 用户目标验证清单

### 必须实现

- 飞书 IM 进度卡片、Web/Admin plan 面板、`/status` 输出与 API `plan_steps` 只展示当前 `update_plan` 快照，不再继承旧 completed 步骤。
- 同一 session 新任务或重规划时，模型可以删除不再相关的旧步骤；同名 completed 可被降级为 `pending` / `in_progress`。
- 同一 turn 中一次 `update_plan` 成功调用只被消费一次，不因后续 shell/read/apply_patch 等工具调用被重复落库或重复推送。
- 空 `plan: []` 只允许在当前无未完成步骤或全部完成后清空；当前 plan 仍有 `pending` / `in_progress` 时忽略空快照并 warn，避免仅收到项目规则/上下文消息时误清。
- 保留 runtime 收口门禁：模型在 plan 未完成时试图 final，`plan_has_unfinished_steps()` 仍要求补一次 `update_plan`。
- `plan_cleared` 仍用于任务完成后新 turn 清空已完成计划。
- Mid-turn/emergency compaction 后同 turn 的下一次模型请求，从 `session.current_plan` 临时恢复当前 plan snapshot 作为 transient developer context，避免模型看不到 plan 而把 11 步重写成 7 步。

### 必须不破坏

- `update_plan` 工具 schema 不变：`plan[]`、`step`、`status`、`explanation` 继续兼容。
- 不改变“最多一个 in_progress”的验证。
- 每次 plan update 仍写入 session JSONL `plan_updated` / `plan_cleared` 事件，审计历史完整。
- IM/Web 展示层不新增自己的 merge/补齐逻辑。
- 不把 Codex 的 plan-mode `proposed_plan` 与 TODO/checklist `update_plan` 混在一起，`session.proposed_plan` 与 `session.current_plan` 保持独立字段。
- Compaction summary 请求与持久化 metadata 不注入 `current_plan` 文本，也不引导 compaction model 保留 completed 历史。

### 必须真实验证

- 单元测试覆盖：快照替换、同名 completed 可降级、缺失 completed 不补回、同一工具调用不重复消费、失败工具结果携带 runtime event 被忽略、mid-turn compaction 后 transient plan context 只落到下一次请求且不写 history。
- E2E 验证：mock 模型分多次提交 plan，最终 API `plan_steps` 严格等于最后一次；session 内新任务 `plan: []` 有条件清空；tool result 文本不再包含 `UPDATE_PLAN:` 前缀。
- 持久化恢复测试：多轮 `plan_updated` + 一次 `plan_cleared` 回放后 `session.current_plan` 与最后事件一致。
- 飞书 IM human_tests 覆盖长任务多次重规划场景，卡片步骤数量随快照替换而非单调增长。

### 必须交付

- 更新本设计文档、`human_tests/agent-plan-lifecycle.md` 与 `human_tests/update-plan.md`，同步 `human_tests/readme.md` 索引。
- 完成至少两轮 Review/Fix/Test 闭环，遵循 `design/agent-development-review-loop.md`。
- 更新 `crates/agent/src/session/turn_loop.rs`、`crates/agent/src/tools/update_plan.rs`、`crates/agent/src/types.rs`、`crates/agent/src/persistence.rs`、`crates/agent/src/session/tests.rs` 与 `crates/bifrost-admin/src/im_gateway/progress_card.rs`。

## 问题结论

### 现象

飞书 M 通道任务中，模型每次 `update_plan` 只提交当前工作所需的 8~10 个步骤，卡片却累积到 20、50 步。旧任务的 completed 项被带到新 plan 里，plan 偏离用户当前目标；mid-turn compaction 之后模型丢失 plan 上下文，重写成粒度更粗的 7 步 plan，Web 面板再次覆盖回旧 completed。

### Bifrost 当前根因

1. `turn_loop.rs::reconcile_plan_update()` 强制把 current 的 completed 项塞回 incoming。
2. `apply_tool_call_completion()` 从 `tool_calls_log.iter().rfind(update_plan)` 反解析 `UPDATE_PLAN:{json}` 字符串信号；`tool_calls_log` 是本 turn 累积日志，后续无关工具调用也会触发同一次 plan update 被反复消费。
3. `ToolResult.output` 携带 `UPDATE_PLAN:{json}` 前缀，一方面污染模型可读输出，一方面让外部测试与 IM 展示层错误地把工具结果字符串当作 runtime 状态源。
4. Compaction summary 请求会把 `current_plan` 拼进 developer 提示；handoff 之后再次 injection 让 completed 步骤沉淀到 summary，永远清不干净。

### Codex 对照结论

- `codex-rs/core/src/tools/handlers/plan.rs` 只解析 `UpdatePlanArgs` 并发送 `EventMsg::PlanUpdate(args)`。
- `codex-rs/protocol/src/plan_tool.rs::UpdatePlanArgs.plan` 就是完整列表，没有 merge 语义。
- `codex-rs/app-server/src/bespoke_event_handling.rs` 把 `PlanUpdate(args)` 转成 `TurnPlanUpdatedNotification` 直接透传。
- `codex-rs/exec/src/event_processor_with_jsonl_output.rs` 对同一 running todo list 做 item update，turn 完成后下一次 plan update 创建新 todo list item。
- Codex core 不帮模型保留旧 completed；模型负责提交完整快照，客户端负责替换展示。

因此 Bifrost 对齐为：`update_plan` = 权威当前快照；`plan_updated` / `plan_cleared` = JSONL 审计事件；`session.current_plan` = runtime 展示态；三者由 `apply_plan_update_snapshot()` 统一维护。

## 产品语义

### `update_plan` 是快照工具，不是差分工具

- 模型每次调用都必须提交“当前完整清单”。
- 模型可以删除已经不相关的步骤（包括 completed）；runtime 不补回。
- 模型可以把某个 completed 步骤重新降级为 pending / in_progress，表示重新规划。
- 空 `plan: []` 是“显式清空”意图，只有当前无未完成步骤时才生效。

### `plan_cleared` 是任务收敛事件，不是通用清空开关

- turn 完成、所有步骤 completed 时，`clear_completed_plan_for_new_turn()` 在下一个 user turn 前落一次 `plan_cleared`。
- 模型主动提交空 plan 且当前无未完成步骤时，也落 `plan_cleared`。
- 其它情况（例如 compaction 之后、guide 消息之后）不允许触发 `plan_cleared`。

### 展示层只消费 `PlanUpdated`

- Web/Admin 顶部 plan 面板、IM 进度卡片 (`progress_card.rs`)、`/status`、API `plan_steps` 只订阅 `AgentTurnProgressEvent::PlanUpdated { steps, title }`。
- 展示层不能自己合并旧 steps。IM 卡片在收到空 steps 时必须清空面板；不能因为“怕闪烁”而保留最后一次非空快照。
- Web history replay 必须消费 `plan_cleared`，避免下一轮对话看到上一轮 completed checklist 固定在 composer 顶部。

## 技术细节

### 运行时 API

在 `crates/agent/src/session/turn_loop.rs`：

```rust
pub(super) fn apply_plan_update_snapshot(
    session: &mut AgentSession,
    recorder: &mut PersistenceRecorder,
    args: UpdatePlanArgs,
) {
    let steps = args.plan;
    if steps.is_empty() {
        let has_unfinished = session
            .current_plan
            .as_deref()
            .map(plan_has_unfinished_steps)
            .unwrap_or(false);
        if has_unfinished {
            warn!(session_id = %session.id, "ignoring empty plan snapshot while unfinished steps exist");
            return;
        }
    }

    session.current_plan = if steps.is_empty() { None } else { Some(steps.clone()) };
    recorder.record_plan_update(&session.id, &steps, args.explanation.as_deref());

    if let Some(sender) = session.turn_progress_sender.as_ref() {
        let _ = sender.send(AgentTurnProgressEvent::PlanUpdated {
            steps: steps.clone(),
            title: session.title.clone(),
        });
    }
}
```

关键位置：

- 定义：`turn_loop.rs:3048`
- 消费 typed event：`apply_tool_runtime_events()` at `turn_loop.rs:462` -> `473: apply_plan_update_snapshot(...)`
- 历史恢复：`turn_loop.rs:1403 session.current_plan = runtime_state.current_plan;`
- Turn end summary：`turn_loop.rs:2466 plan_steps: session.current_plan.clone();`

### 工具调用消费从字符串信号迁移到 typed runtime event

在 `crates/agent/src/types.rs` 新增：

```rust
pub enum ToolRuntimeEvent {
    PlanUpdate(UpdatePlanArgs),
}

pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub runtime_events: Vec<ToolRuntimeEvent>,
}
```

`crates/agent/src/tools/update_plan.rs::UpdatePlanTool::execute()`：

- 成功时返回 `ToolResult { success: true, output: "Plan updated".into(), runtime_events: vec![ToolRuntimeEvent::PlanUpdate(args)] }`。
- 失败时 `runtime_events` 为空。
- 不再写 `UPDATE_PLAN:{json}` 字符串前缀。

`turn_loop.rs::apply_tool_runtime_events()`：

- 只消费当前 `ToolResult.runtime_events`，不重扫 `tool_calls_log`。
- 防御性丢弃 `success == false` 结果携带的 runtime event。
- 顺序：`execute tool -> record ToolResult -> drain runtime_events -> apply_plan_update_snapshot -> persist plan_updated / plan_cleared -> push PlanUpdated -> append ToolCallCompleted`。

`extract_plan_update_from_tool_result()` 与相关字符串 parser 全部删除。共享 `parse_update_plan_arguments()` 由工具和 runtime event 派发共用同一 parser（Phase 1 完成后置于 `tools/update_plan.rs`）。

### 历史审计与展示态分离

- `plan_updated`：`session.current_plan = event.plan`。
- `plan_cleared`：`session.current_plan = None`。
- `compaction_performed`：不写入、不读取、不恢复、不清空 `current_plan`；`compact.rs`、`compact_tests.rs`、`compaction_hooks.rs` 均不得触碰 plan state。
- session JSONL 恢复走 `crates/agent/src/persistence.rs::replay_events()`，遵循“最后有效状态”策略。

### Mid-turn compaction 后的 transient plan context

- 只在 `turn_loop.rs` 内部已经发生 mid-turn / emergency compaction 且下一次模型请求即将发出时启用。
- 内容作为 developer runtime 提示：`"当前 update_plan 快照（compaction 后恢复）: [...]，调用 update_plan 时仍提交完整当前快照。"`
- 不写入 `session.history`、不写 JSONL、不进 compaction summary 请求。
- 不跨新 user turn 注入；`queued_continuation_compacts_before_next_model_request` 场景返回“已压缩”信号后仍可启用。
- 单元测试 `mid_turn_plan_context_is_transient_model_context` 与 `queued_continuation_compacts_before_next_model_request` 守住边界。

### 收口门禁

保留 `plan_has_unfinished_steps()` / `plan_is_complete()`。模型试图 final 时 runtime 注入提示：

```text
Plan 仍有未完成步骤。请提交当前任务的完整最终快照（只包含仍相关的步骤，不需要保留历史执行日志）。
```

### 关键文件

- `crates/agent/src/tools/update_plan.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/session/tests.rs`
- `crates/agent/src/types.rs`
- `crates/agent/src/session_status.rs`
- `crates/agent/src/persistence.rs`
- `crates/agent/src/compact.rs`
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`
- `crates/bifrost-admin/src/handlers/agent_chat.rs`（`planSteps` 字段序列化）
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`
- `apps/web/src/**`（plan 面板渲染，禁止本地 merge）

## CLI / Web / Admin API

### CLI

- `bifrost agent status` / `/status`：输出 `Plan: <steps count>`，直接读取 `session.current_plan`；空 plan 显示 `Plan: (none)`。
- CLI 不新增 `plan` 子命令；模型层控制 plan lifecycle。

### Web / Admin

- `GET /api/agent/sessions/:id` 响应 `plan_steps` 与最后一次 `PlanUpdated` 严格一致；`plan_steps: null` 表示无当前 plan。
- SSE `agent.turn.progress` 中 `PlanUpdated { steps, title }` 每次 plan update 都推送；展示层直接替换。
- 顶部 plan 面板收到 `steps == []` 或 `plan_cleared` 时清空；不再保留“最后一次非空” fallback。
- IM 进度卡片：`progress_card.rs::PlanUpdated` handler 直接 `self.plan_steps = steps;`；不做 merge。

### Admin API 边界

- `POST /api/agent/chat/stream` 与 `POST /_bifrost/api/im-gateway/agent/chat` 走同一 turn_loop，plan 语义一致。
- `PUT /api/agent/sessions/:id/plan` 不新增。所有 plan 变更来自模型 `update_plan` 工具调用。
- Admin/Web 侧断言 `tool_calls[].result` 不再包含 `UPDATE_PLAN:`；`tool_calls[].arguments` 仍保留原始 `update_plan` 参数便于审计。

## Sync 边界

- `session.current_plan` 属于本地 runtime 状态，不参与远端 rule / group / value sync。
- session JSONL history（含 `plan_updated` / `plan_cleared`）属于本地会话存储，不通过 sync 上传。
- 若未来在多设备之间恢复同一 session，仍以本地 JSONL replay 为准，不依赖 sync 中间态。

## 实现切分

### Phase 1：Runtime plan 语义快照替换

- 新增 `apply_plan_update_snapshot()`、删除 `reconcile_plan_update()`。
- 更新 `apply_tool_call_completion()` 只读 `ToolResult.runtime_events`。
- 恢复路径 `runtime_state.current_plan` 遵循最后事件。
- 单元测试：`test_apply_plan_update_snapshot_replaces_completed_history` 等（`session/tests.rs:152` 及后续用例）。

### Phase 2：ToolResult typed runtime event

- `crates/agent/src/types.rs` 增加 `ToolRuntimeEvent` / `ToolResult.runtime_events`。
- `UpdatePlanTool::execute()` 返回 typed event，`output` 改为 `Plan updated`。
- 删除 `extract_plan_update_from_tool_result()` 与相关测试。
- 全 workspace 搜索 `UPDATE_PLAN:` 前缀，确保只剩兼容注释。

### Phase 3：展示层与 compaction 边界

- IM `progress_card.rs`、Web plan 面板、Admin API `plan_steps` 直接消费当前 `steps`。
- Compaction summary 请求禁止拼接 `current_plan`。
- Mid-turn compaction 后启用 transient plan context 提示，单元测试守边界。

### Phase 4：文档、E2E、human_tests

- 更新本设计与 `human_tests/agent-plan-lifecycle.md`、`human_tests/update-plan.md`、`human_tests/readme.md`。
- 更新 `e2e-tests/tests/test_update_plan_human_api.sh` 覆盖三轮 plan 快照、mock final 门禁、tool result 文本断言。
- 更新 CI wrapper：`scripts/ci/local-ci.sh --e2e-only shell` 之前必须先 `cargo build --release -p bifrost`，然后 E2E 用 `--skip-build`。

## 测试方案

### 单元测试（`crates/agent/src/session/tests.rs`）

- `test_apply_plan_update_snapshot_replaces_completed_history`
- `test_apply_plan_update_snapshot_allows_completed_step_to_reopen`
- `plan_update_is_processed_once_per_tool_call`
- `multiple_plan_updates_in_one_turn_are_applied_in_order`
- `plan_cleared_still_resets_completed_plan_on_new_turn`
- `empty_plan_snapshot_clears_current_plan`
- `empty_plan_snapshot_does_not_clear_unfinished_plan`
- `persistence_replay_uses_last_plan_snapshot`
- `compaction_does_not_mutate_plan_runtime_state`
- `mid_turn_plan_context_is_transient_model_context`
- `queued_continuation_compacts_before_next_model_request`
- `update_plan_tool_output_is_plain_text`
- `update_plan_runtime_event_is_returned_by_tool_result`
- `failed_update_plan_does_not_emit_runtime_event`
- `failed_tool_result_runtime_events_are_ignored`

### E2E 测试

- `e2e-tests/tests/test_update_plan_human_api.sh`：
  - Round 1 提交 plan A（8 步），Round 2 提交 plan B（6 步不重叠），断言 API `plan_steps == B`。
  - Round 3 提交 plan C 表示新任务，断言不包含 A/B 中已删除的 completed。
  - Round 4 提交 `plan: []` 在当前 plan 全部 completed 后清空，断言 API `plan_steps == null` 且 IM 卡片收到空快照。
  - Round 5 模型在 plan 未完成时直接 final，断言 runtime 注入 “补 final snapshot” 提示。
  - 断言所有 tool result 文本不再包含 `UPDATE_PLAN:`；`tool_calls[].arguments` 仍含原始参数。
- `e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh`：外置 Runner plan UI 与内置 Agent 行为一致。

### 真实场景测试（human_tests）

- 更新 `human_tests/update-plan.md`（当前 126 行）：覆盖真实 Bifrost + Admin API + mock model server 的 plan 快照替换。
- 更新 `human_tests/agent-plan-lifecycle.md`（当前 181 行）：
  - `TC-APL-01` 快照替换。
  - `TC-APL-02` 同一 turn 多次 plan update 顺序覆盖。
  - `TC-APL-03` 后续无关工具调用不重复消费上一 plan update。
  - `TC-APL-04` 空 plan 仅在无未完成步骤时清空。
  - `TC-APL-05` mid-turn compaction 后 transient plan context 只在下一次模型请求生效。
  - `TC-APL-06` 飞书 M 通道长任务重规划，卡片步骤数量不单调增长。
  - `TC-APL-07` 持久化恢复：多个 `plan_updated` + `plan_cleared` 回放后 `current_plan` 与最后事件一致。
- 更新 `human_tests/readme.md` 索引；禁止维护全局汇总数字。

### Coverage 与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-agent --all-features plan_`
- `cargo test -p bifrost-admin im_gateway_progress_card`
- `bash e2e-tests/tests/test_update_plan_human_api.sh`
- `bash e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机若维持 no-local-coverage 约定，可跳过 `make coverage` / `make coverage-unit`，交付时说明依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：防漂移、防额外增长、Codex 对齐、mid-turn compaction 后 transient plan context。
- 执行 `git status --short --branch`、`git diff` 与必要 `git diff --cached`。
- Review `turn_loop.rs`、`update_plan.rs`、`persistence.rs`、`progress_card.rs`、`compact.rs`。
- 关键点：是否仍存在 completed-preserve 语义；是否仍 `rfind(update_plan)` 重复消费；typed event 是否被 `success == false` 结果污染；compaction summary 是否拼 `current_plan`。
- 复测：`cargo test -p bifrost-agent --all-features plan_`、`bash e2e-tests/tests/test_update_plan_human_api.sh`。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff，人工确认：
  - `UPDATE_PLAN:` 字符串已全部清理。
  - `ToolResult.runtime_events` 只在成功结果 drain。
  - IM `progress_card.rs::plan_steps = steps` 无 merge 分支。
  - Web plan 面板收到 `plan_cleared` 后清空。
  - `human_tests/readme.md` 索引与新用例编号一致。
- 复跑 targeted 单元、`test_update_plan_human_api.sh`、`test_im_gateway_external_runner_plan_ui.sh` 与 workspace 兜底。
- 如果仍有语义漂移或测试只断言“包含旧步骤”，追加第 3 轮。

## 风险与决策

- 模型主动列出过细步骤时 runtime 无法自动收敛粒度；只能靠 prompt 约束（`crates/agent/src/prompts/` 中 base instructions）与最终 answer 证据台账承担详细执行历史。
- IM 通道 card 更新失败时可能退化为新消息发送，用户看到旧卡片残留；这属于消息更新可靠性问题，不通过 plan merge 解决，走 `design/im-agent-streaming-card.md` 修复路径。
- typed runtime event 迁移改变 `update_plan` 工具结果文本；外部测试或调用方若错误依赖 `UPDATE_PLAN:` 前缀，需改为读取 `tool_calls[].arguments`、`plan_updated` / `plan_cleared` JSONL 事件或 API `plan_steps`。
- Phase 1 只迁移 `update_plan`。`set_title`、`switch_workdir` 等 side-effect 工具是否迁移到 typed runtime event，留在 Phase 2 评估；防止 Phase 1 顺手扩大范围。
- Compaction 内引入 transient plan context 是一个 turn-local 提示，不写 history，不写 JSONL；若未来接入 Codex plan-mode `proposed_plan`，必须保持 `session.current_plan` 与 `session.proposed_plan` 独立，不能复用 transient 恢复路径互相污染。
