# Agent Plan Lifecycle

## 功能模块说明

本方案用于修复 Bifrost Agent `update_plan` 在飞书 IM 进度卡片中持续膨胀、偏离用户原始目标的问题。目标是参考 `~/work/github/codex` 的设计，把 plan 从“历史累计列表”改回“当前任务快照”。

本轮按本文约束修改 runtime 代码，并补齐单元、E2E 与 human_tests 真实链路验证。

## 问题结论

### 现象

飞书 M 通道任务中，模型每次调用 `update_plan` 本来只提交当前工作所需的 8 到 10 个步骤，但最终卡片显示会累积到 20、50 甚至更多步骤。旧任务的 completed 项被带到新计划里，导致用户看到的 plan 逐步偏离当前目标。

### Bifrost 当前根因

1. `crates/agent/src/session/turn_loop.rs` 在每轮工具批结束后从 `tool_calls_log.iter().rfind(update_plan)` 找最近一次成功 `update_plan`。
2. 找到后调用 `reconcile_plan_update(session.current_plan.as_deref(), args.plan)`。
3. `reconcile_plan_update()` 会：
   - 对同名旧 completed 步骤强制保持 completed。
   - 当 incoming completed 数量少于 current completed 数量时，把 current 中缺失的 completed 步骤补回 incoming。
4. 这个逻辑把 plan 变成“已完成事项追加日志”，而不是“当前任务状态快照”。
5. 因为 `tool_calls_log` 是本 turn 累积日志，后续无关工具调用也可能反复重新消费同一个最近 `update_plan`，进一步放大旧状态影响。

### Codex 对照结论

Codex 的 `update_plan` 行为更简单：

- `codex-rs/core/src/tools/handlers/plan.rs` 只解析工具参数并发送 `EventMsg::PlanUpdate(args)`。
- `codex-rs/protocol/src/plan_tool.rs` 中 `UpdatePlanArgs.plan` 是完整列表，没有历史 merge 语义。
- `codex-rs/app-server/src/bespoke_event_handling.rs` 把 `PlanUpdate(args)` 转成 `TurnPlanUpdatedNotification`，直接把本次 plan 透传给客户端。
- `codex-rs/exec/src/event_processor_with_jsonl_output.rs` 对同一个 running todo list 执行 item update；turn 完成后下一次 plan update 会创建新的 todo list item。
- Codex 不在 core runtime 中“帮模型保留旧 completed 项”。模型负责提交完整当前 plan，客户端负责替换展示。

因此，Bifrost 应对齐为：`update_plan` 是权威当前快照；历史由 session JSONL 事件保存，不参与下一次展示态合成。

## 用户目标验证清单

### 必须实现

- 飞书 IM 进度卡片展示当前 `update_plan` 快照，不再额外继承旧 completed 步骤。
- 同一 session 新任务或重规划时，允许模型删除不再相关的旧步骤。
- 同一 turn 中一次 `update_plan` 成功调用只消费一次，不因后续工具调用被重复落库或重复推送。
- 保留 runtime 强制收口能力：最终结束前仍可要求未完成 plan 补成 completed。
- `plan_cleared` 仍用于任务完成后新 turn 清空已完成计划。

### 必须不破坏

- 不破坏 `update_plan` 工具 schema：`plan[]`、`step`、`status`、`explanation` 继续兼容。
- 不改变“最多一个 in_progress”的验证。
- 不丢失历史审计：每次 plan update 仍记录到 session JSONL。
- 不让飞书卡片展示层自行 merge 旧计划。
- 不把 Codex 的 plan-mode proposed plan 与 TODO/checklist `update_plan` 混在一起。

### 必须真实验证

- 单元测试覆盖快照替换、同名 completed 可降级、缺失 completed 不补回、同一工具调用不重复消费。
- E2E/API 测试覆盖 mock model 分多次提交 plan，最终 API `plan_steps` 严格等于最后一次 plan 快照。
- 飞书 IM human_tests 覆盖长任务中多次重规划，卡片步骤数量不随历史增长。
- 持久化恢复测试覆盖 `plan_updated` / `plan_cleared` 回放后的 `current_plan` 是最后状态。

### 必须交付

- 更新设计文档。
- 更新 `human_tests/` 用例与索引。
- 实现时同步更新既有 `human_tests/update-plan.md` 或新增更细分用例。
- 完成至少两轮 Review/Fix/Test。

## 改造方案

### 1. Runtime plan 语义改为快照替换

删除 `reconcile_plan_update()` 的 completed 继承语义。新的行为：

```text
current_plan = if incoming_plan.is_empty() { None } else { incoming_plan }
```

要求：

- incoming 为空表示当前任务不再需要展示 plan，runtime 清空 `current_plan`、发送空 `PlanUpdated` 清空展示，并持久化 `plan_cleared`。
- 同名步骤从 completed 变回 pending/in_progress 是允许的，表示模型重新规划。
- 缺失步骤表示该步骤不属于当前计划，不能补回。

实现 helper 使用 `apply_plan_update_snapshot()` 命名，避免 `reconcile` 暗示 merge。

### 2. 工具调用消费从“全量 rfind”改成“typed runtime event”

当前每轮工具批结束后扫描全量 `tool_calls_log`，容易重复消费旧的成功 update。改造方向：

- 不再让 `update_plan` 工具把结构化参数塞进 `ToolResult.output` 的 `UPDATE_PLAN:{json}` 字符串前缀。
- `ToolResult.output` 只返回模型可读的稳定文本，例如与 Codex 对齐的 `Plan updated`。
- 在 `UpdatePlanTool::execute()` 内部解析 `UpdatePlanArgs`，并随 `ToolResult.runtime_events` 返回 `ToolRuntimeEvent::PlanUpdate(args)`。
- `apply_tool_call_completion()` 只消费当前 `ToolResult.runtime_events` 中的 typed event，立即调用 `apply_plan_update_snapshot()`。
- 对 `set_title` 可沿用 latest-wins，但 `update_plan` 必须保证一次工具调用只落库一次。

推荐内部接口：

```rust
enum ToolRuntimeEvent {
    PlanUpdate(UpdatePlanArgs),
}

struct ToolResult {
    success: bool,
    output: String,
    runtime_events: Vec<ToolRuntimeEvent>,
}
```

工具完成时的顺序变为：

```text
execute tool
tool parses its own arguments and returns ToolResult.runtime_events
record normal tool result for model/logging
drain typed runtime events from this tool result only
apply PlanUpdate snapshot / persist plan_updated or plan_cleared / push PlanUpdated display event
append ToolCallCompleted turn event
```

这样可以自然支持一个工具批内多次 `update_plan` 按顺序覆盖展示，同时不会在后续 shell/read 工具结束后重放旧 plan。

这个设计比 `UPDATE_PLAN:` 字符串信号更贴近 Codex 的架构：Codex handler 解析 `UpdatePlanArgs` 后发送 `EventMsg::PlanUpdate(args)`；Bifrost 因为还需要现有 `ToolHandler -> ToolResult` 抽象和授权编排，让工具把 typed event 放进 `ToolResult.runtime_events`，再由 turn loop 统一消费。

实施分两步，避免一次重构过大：

1. Phase 1 只迁移 `update_plan`：
   - 新增共享 `parse_update_plan_arguments()`，工具执行和 runtime event 派发共用同一 parser。
   - `UpdatePlanTool::execute()` 成功时返回纯文本 `Plan updated`，并在 `ToolResult.runtime_events` 中返回 `ToolRuntimeEvent::PlanUpdate(args)`，不再返回 `UPDATE_PLAN:{json}`。
   - 删除 `extract_plan_update_from_tool_result()` 和相关测试。
   - `apply_tool_call_completion()` 不再解析 `tc.arguments()` 生成 plan event，只消费当前 `ToolResult.runtime_events`。
2. Phase 2 再评估 `set_title` / `switch_workdir` 等 side-effect 工具是否也迁移到 typed runtime event。不要在 Phase 1 顺手扩大范围。

边界要求：

- typed event 不写入 `ToolCallLog.result`，避免展示层或审计层误把内部状态当模型输出。
- typed event 不进入模型 history；模型只看到普通工具结果文本。
- `update_plan` 仍保持 ordered execution，不能进入 parallel tool batch。
- 如果工具参数解析失败，`UpdatePlanTool::execute()` 返回失败结果且 `runtime_events` 为空。
- `apply_tool_call_completion()` 必须防御性忽略 `success == false` 的 result 中携带的 runtime events，避免未来工具误填事件污染 runtime state。

### 3. 历史审计与展示态分离

保留 session JSONL 中每次 `plan_updated` 事件，作为审计历史。恢复 runtime 时使用“最后一个有效状态”：

- `plan_updated`：`current_plan = event.plan`
- `plan_cleared`：`current_plan = None`
- `compaction_performed.current_plan`：仅作为 compact 后状态快照，不对旧 plan 做 merge

展示层只消费 `AgentTurnProgressEvent::PlanUpdated { steps, title }` 的 `steps` 当前值。`crates/bifrost-admin/src/im_gateway/progress_card.rs` 不应保存或补齐旧步骤。

Compaction prompt 必须继续参考 Codex：handoff summary 只描述当前进展、关键上下文和剩余工作，不额外向 compaction model 注入 `current_plan` 文本。`current_plan` 只作为结构化 runtime state 写入 JSONL/compaction metadata，用于恢复 UI 和 runtime gate；不要让 summary 阶段重新解释 checklist。

### 4. Runtime 收口门禁保留但不增殖

现有 `plan_has_unfinished_steps()` / `plan_is_complete()` 可以保留。模型试图在 plan 未完成时结束，runtime 仍可注入提示要求补一次 `update_plan`。

收口提示需要改成“请提交当前任务的完整最终快照”，避免诱导模型把全部历史动作列出来。推荐文案原则：

- 要求包含当前任务仍相关的步骤。
- 不要求保留不再相关的历史步骤。
- 不要求把所有执行过的工具动作都列成 plan 项。

### 5. Prompt 约束收敛 plan 粒度

在 base instructions 或工具说明中补充简短约束：

- Plan 是当前任务的短清单，不是执行日志。
- 更新 plan 时提交完整当前清单。
- 当目标改变、子任务收敛或发现原计划不合适时，可以删除或重写步骤。
- 避免把启动检查、每轮 review、每条测试命令都长期保留在同一 plan 中；最终交付证据放 final，不放进 plan。
- Compaction / handoff prompt 不得要求保留 completed 历史步骤，也不得额外注入 plan 文本；这些状态只属于结构化 runtime state 和审计记录，不属于 summary 的自然语言任务描述。

这部分只用于减少模型漂移；真正的防膨胀必须靠 runtime 快照语义保证。

## 依赖项

- `crates/agent/src/tools/update_plan.rs`
- `crates/agent/src/session/turn_loop.rs`
- `crates/agent/src/session/tests.rs`
- `crates/agent/src/types.rs`
- `crates/agent/src/session_status.rs`
- `crates/agent/src/persistence.rs`
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`
- `crates/bifrost-admin/src/handlers/agent_chat.rs`
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`
- `human_tests/update-plan.md`
- `human_tests/agent-plan-lifecycle.md`

## 测试方案

### 单元测试

- `plan_snapshot_replaces_previous_completed_steps`：current 有多个 completed，incoming 只包含新任务，结果严格等于 incoming。
- `plan_snapshot_allows_completed_step_to_be_reopened`：同名 completed 输入为 pending 后保持 pending。
- `plan_update_is_processed_once_per_tool_call`：同一轮后续普通工具调用不会重复 record/send 上一次 `update_plan`。
- `multiple_plan_updates_in_one_turn_are_applied_in_order`：同一 turn 两次 plan update，最终 `current_plan` 等于第二次。
- `plan_cleared_still_resets_completed_plan_on_new_turn`：完成后下一轮仍清空。
- `empty_plan_snapshot_clears_current_plan`：模型提交 `plan: []` 时清空 runtime state、发送空展示快照，并恢复为无当前 plan。
- `persistence_replay_uses_last_plan_snapshot`：多个 `plan_updated` 回放后只保留最后一个。
- `update_plan_tool_output_is_plain_text`：成功工具结果为 `Plan updated`，不包含 `UPDATE_PLAN:` 或参数 JSON。
- `update_plan_runtime_event_is_returned_by_tool_result`：typed event 由 `UpdatePlanTool::execute()` 解析参数后放入 `ToolResult.runtime_events`，而不是从 `ToolResult.output` 或 completion 阶段反解析。
- `failed_update_plan_does_not_emit_runtime_event`：工具失败时不产生 `PlanUpdate`，不会污染 `current_plan`。
- `failed_tool_result_runtime_events_are_ignored`：防御性覆盖失败结果即使携带 runtime event 也不更新 runtime state。

### E2E 测试

- 更新 `e2e-tests/tests/test_update_plan_human_api.sh`：
  - 第一轮提交 plan A，后续提交 plan B，断言最终 API `plan_steps == B`。
  - 第二轮同 session 提交新任务 plan C，断言不包含 A/B 中已删除的 completed 项。
  - 第三轮提交 `plan: []`，断言最终 API `plan_steps == null`，并且展示层收到空快照用于清空 plan。
  - mock model 在 plan 未完成时直接 final，断言 runtime 仍要求补 final snapshot。
  - 断言 tool result 文本不再包含 `UPDATE_PLAN:`，但 API `plan_steps` 与 IM display event 仍保持正确。
  - 断言 `tool_calls[].arguments` 仍保留原始 `update_plan` 参数，便于审计；runtime 状态不依赖 `tool_calls[].result` 解析。
- 本地 CI 在执行 rules/shell/platform E2E 前必须先构建当前源码的 release `bifrost`，再让各 E2E wrapper 使用 `--skip-build`。否则 `test_update_plan_human_api.sh` 这类真实服务用例可能消费旧 `target/release/bifrost`，把已删除的 `UPDATE_PLAN:` 字符串信号误判为当前实现行为。

### 真实场景测试

- 更新 `human_tests/update-plan.md`，覆盖真实 Bifrost + Admin API + mock model server。
- 新增 `human_tests/agent-plan-lifecycle.md`，覆盖本设计和 runtime 实现的静态验收入口。
- 飞书 IM 真链路执行时，观察进度卡片步骤数应随当前快照替换，不随历史轮次单调增长。
- 本地 `scripts/ci/local-ci.sh --e2e-only shell` 必须先出现 `cargo build (release bifrost)` 并通过，然后再执行 shell E2E，确保真实 Bifrost 服务来自当前源码。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复读用户目标：防 plan 漂移、防额外增长、参考 Codex 当前快照设计。
- 执行 `git status --short`、`git diff`。
- Review `turn_loop.rs`、`update_plan.rs`、`persistence.rs`、IM card 展示路径。
- 运行 `cargo test -p bifrost-agent --all-features plan_` 与更新后的 targeted E2E。
- 如发现 merge 语义残留或测试只断言“包含旧步骤”，立即修复。

### 第 2 轮

- 复查第 1 轮修复后的 diff，重点检查：
  - 是否仍有 completed-preserve 语义。
  - 是否仍从全量 `tool_calls_log.rfind(update_plan)` 重复消费。
  - human_tests/readme 索引是否同步。
  - 飞书卡片是否只替换当前 steps。
- 复跑 targeted 单元、E2E 和 human_tests。
- 如第 2 轮仍发现缺口，继续追加第 3 轮。

## 校验要求

实现任务收尾前必须执行：

- `cargo test -p bifrost-agent --all-features plan_`
- 更新后的 update-plan E2E 脚本
- 对应 human_tests 逐条执行
- `cargo test --workspace --all-features`
- rust-project-validate

仅设计文档变更时，Rust/E2E/workspace 校验可标记不适用；本轮包含 runtime 改造，必须执行 targeted 单元、E2E、human_tests 和 workspace 适用校验。

## 残余风险

- 如果模型在单次快照中主动列出过多步骤，runtime 快照语义只能防“历史额外增长”，不能自动判断语义粒度是否过细。需要 prompt 约束和最终 answer 证据台账承担详细历史。
- 如果下游 IM 通道在 card 更新失败后退化为新消息发送，用户可能看到旧卡片残留；这属于消息更新可靠性问题，不应通过 plan merge 解决。
- typed runtime event 迁移会改变 `update_plan` 的工具结果文本；如果外部测试或调用方错误依赖 `UPDATE_PLAN:` 前缀，需要同步改为读取 tool call arguments、`plan_updated`/`plan_cleared` JSONL 事件或 API `plan_steps`。
- Phase 1 只迁移 `update_plan`，因此短期内 Bifrost 仍同时存在普通工具文本输出和少量 side-effect 后处理逻辑；必须用测试防止未来重新引入从 `ToolResult.output` 解析 plan 的捷径。
