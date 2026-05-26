# Agent Plan 生命周期测试用例

## 功能模块说明

本模块验证 Agent `update_plan` 生命周期从“历史累计”改为“当前快照”的设计与实现。本文档先覆盖设计与代码静态验收；真实 Bifrost + Admin API 链路由 `human_tests/update-plan.md` 的 TC-UP-02 执行。

## 前置条件

1. 当前目录位于仓库根目录。
2. 静态验收命令用于确认设计、runtime 代码和 human_tests 索引一致。
3. 真实 runtime 验收必须执行 `human_tests/update-plan.md` 的 TC-UP-02，启动真实 Bifrost 服务且携带 `--no-system-proxy`。

## 测试用例列表

### TC-APL-01：设计方案明确 Codex 快照语义

**操作步骤**：
1. 执行：
   ```bash
   test -f design/agent-plan-lifecycle.md
   rg -n "Codex 对照结论|权威当前快照|current_plan = incoming_plan|不再额外继承旧 completed 步骤" design/agent-plan-lifecycle.md
   ```

**预期结果**：
- `design/agent-plan-lifecycle.md` 存在。
- 文档明确说明 Codex `update_plan` 是当前快照。
- 文档明确禁止把旧 completed 步骤补回新计划。

### TC-APL-02：runtime 代码移除历史 merge 与重复消费风险

**操作步骤**：
1. 执行：
   ```bash
   rg -n "全量 rfind|只扫描本批新增 tool call|一次工具调用只落库一次|不会在后续 shell/read 工具结束后重放旧 plan" design/agent-plan-lifecycle.md
   rg -n "apply_tool_runtime_events|apply_plan_update_snapshot|ToolRuntimeEvent::PlanUpdate" crates/agent/src/session/turn_loop.rs
   rg -n "runtime_events: vec!\\[ToolRuntimeEvent::PlanUpdate\\(args\\)\\]|output: \"Plan updated\"" crates/agent/src/tools/update_plan.rs
   if rg -n "reconcile_plan_update|rfind\\(\\|l\\| l.tool_name == \"update_plan\"" crates/agent/src/session/turn_loop.rs; then exit 1; fi
   if rg -n "extract_plan_update_from_tool_result|UPDATE_PLAN:" crates/agent/src; then exit 1; fi
   ```

**预期结果**：
- 文档明确指出当前全量 `rfind(update_plan)` 的重复消费风险。
- 文档给出按新增工具调用顺序消费 `update_plan` 的改造方案。
- runtime 代码中存在 `apply_tool_runtime_events` / `apply_plan_update_snapshot`，并只消费当前工具结果携带的 typed runtime event。
- `update_plan` 工具成功输出为 `Plan updated`，并通过 `ToolResult.runtime_events` 携带 `PlanUpdate`。
- runtime 代码中不存在 `reconcile_plan_update` 和全量 `rfind(update_plan)` 消费逻辑。
- runtime 代码中不存在 `UPDATE_PLAN:` 字符串信号解析逻辑。

### TC-APL-03：设计方案覆盖实现验证矩阵

**操作步骤**：
1. 执行：
   ```bash
   rg -n "plan_snapshot_replaces_previous_completed_steps|plan_update_is_processed_once_per_tool_call|multiple_plan_updates_in_one_turn_are_applied_in_order|test_update_plan_human_api.sh|飞书 IM 真链路" design/agent-plan-lifecycle.md
   ```

**预期结果**：
- 文档列出单元测试、E2E 和飞书 IM human_tests 的验证点。
- 验证点覆盖快照替换、同名 completed 降级、同一调用不重复消费、多次 plan update 按序覆盖。

### TC-APL-04：human_tests 索引同步

**操作步骤**：
1. 执行：
   ```bash
   rg -n "agent-plan-lifecycle.md|Agent Plan 生命周期" human_tests/readme.md
   ```

**预期结果**：
- `human_tests/readme.md` 已收录本测试文档。
- 索引中的用例数与本文档一致。

### TC-APL-05：compaction prompt 不注入 plan 自然语言

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'Compaction prompt 必须继续参考 Codex|不额外向 compaction model 注入 `current_plan` 文本|不要让 summary 阶段重新解释 checklist' design/agent-plan-lifecycle.md
   rg -n 'test_build_compaction_messages_does_not_inject_plan_text|test_codex_compaction_templates_are_exact' crates/agent/src/compact.rs
   ! rg -n 'preserving completed work|do not regress completed steps|latest current snapshot|not a historical checklist|do not carry completed steps forward|Current persisted task plan before compaction' crates/agent/src/compact.rs
   ```

**预期结果**：
- 设计文档明确 compaction 继续参考 Codex，不向 compaction model 注入 plan 文本。
- `compact.rs` 的测试覆盖 compaction request 不包含 plan 自然语言消息。
- `compact.rs` 中不存在会诱导模型保留或丢弃 completed 历史步骤的 Bifrost-only plan prompt。

### TC-APL-06：空 plan 快照清空当前 plan

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'incoming 为空表示当前任务不再需要展示 plan|record_plan_cleared|第三轮提交 `plan: \\[\\]`' design/agent-plan-lifecycle.md human_tests/update-plan.md
   rg -n 'test_apply_plan_update_empty_snapshot_clears_plan|test_empty_plan_allowed_as_clear_snapshot|record_plan_cleared' crates/agent/src/session/tests.rs crates/agent/src/tools/update_plan.rs crates/agent/src/session/turn_loop.rs
   ```

**预期结果**：
- 设计文档明确 `plan: []` 是清空当前快照，不是非法输入。
- 单元测试覆盖空 plan 清空 runtime state、progress card 空快照和持久化恢复。
- 真实 API human test 覆盖第三轮 `plan: []` 清空后 `plan_steps == null`。

### TC-APL-07：typed PlanUpdate runtime event 方案验收

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'typed runtime event|ToolRuntimeEvent::PlanUpdate|UPDATE_PLAN|ToolResult.output|ToolResult.runtime_events|Phase 1 只迁移 `update_plan`' design/agent-plan-lifecycle.md
   rg -n 'update_plan_tool_output_is_plain_text|update_plan_runtime_event_is_returned_by_tool_result|failed_update_plan_does_not_emit_runtime_event|failed_tool_result_runtime_events_are_ignored' design/agent-plan-lifecycle.md
   ! rg -n 'runtime_events_for_tool_call|基于当前 `tc.name\\(\\)`、`tc.arguments\\(\\)`、`result.success` 生成' design/agent-plan-lifecycle.md
   ```

**预期结果**：
- 设计文档明确下一步把 `UPDATE_PLAN:` 字符串信号迁移为 typed `PlanUpdate` runtime event。
- 设计文档明确 `ToolResult.output` 只保留模型可读文本，不再承载内部 plan JSON。
- 设计文档明确 runtime event 由 `UpdatePlanTool::execute()` 放入 `ToolResult.runtime_events`，而不是由 completion 阶段重新解析 tool call arguments 生成。
- 设计文档明确失败工具调用不得产生 `PlanUpdate`，且 completion 阶段要防御性忽略失败 result 携带的 runtime event。
- 设计文档列出禁止未来重新依赖 `ToolResult.output` 解析 plan 的单元/E2E 验证点。

### TC-APL-08：local-ci 使用当前源码 release 二进制执行真实服务 E2E

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'cargo build \\(release bifrost\\)|NEEDS_RELEASE_BUILD|SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost' scripts/ci/local-ci.sh
   rg -n '本地 CI 在执行 rules/shell/platform E2E 前必须先构建当前源码的 release `bifrost`|真实 Bifrost 服务来自当前源码' design/agent-plan-lifecycle.md
   ```
2. 执行：
   ```bash
   scripts/ci/local-ci.sh --skip-static --skip-deps-audit --e2e-only shell --shard 100/112
   ```

**预期结果**：
- `local-ci.sh` 在 rules/shell/platform E2E 前执行 `cargo build (release bifrost)`。
- `--skip-static --e2e-only shell --shard 100/112` 仍会构建 release `bifrost`，避免 shell E2E 使用 stale `target/release/bifrost`。
- 该 shard 执行 `test_update_plan_human_api.sh` 并通过，证明 `--skip-build` 的 shell E2E 使用的是刚构建的当前源码二进制。

## 清理步骤

1. 静态验收不会创建临时进程或数据目录，无需清理。
2. 后续真实服务验收如创建临时 `BIFROST_DATA_DIR`，必须在测试结束后删除。
