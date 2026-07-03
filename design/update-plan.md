# update_plan 工具设计

## 背景

Bifrost agent 早期的 `update_plan` 工具是一个“看上去像有计划”的展示信号:tool 返回 `UPDATE_PLAN:{json}` 字符串,turn loop 扫最后一次成功调用,把结果临时塞进 `TurnResult.plan_steps`,IM Gateway 再用这个临时结果渲染卡片。这带来了两个真实问题:

1. 计划不是 session 级状态。跨 iteration 只有临时局部变量保存最后一次 plan,一旦模型下一 turn 忘记再调,plan 语义就丢了。
2. 最终回答前没有强制收口。模型说“任务完成”时,plan 可能还有 `pending` / `in_progress` 步骤,IM 卡片上就会看到“Agent 说完成了,但计划里还有 3 步没做完”。

本设计把 `update_plan` 从**单 turn 展示信号**升级为 **runtime 持有的会话状态**,并加了强制收口 + 新任务边界清理两条闭环。计划语义仍由模型负责,但生命周期由 runtime 负责。

代码入口:

- `crates/agent/src/tools/update_plan.rs` — 工具实现(校验 + `ToolRuntimeEvent::PlanUpdate`)。
- `crates/agent/src/session.rs` — `AgentSession.current_plan` / `plan_repair_attempts` 状态。
- `crates/agent/src/session/turn_loop.rs` — turn loop 闭环、`plan_has_unfinished_steps`、`clear_completed_plan_for_new_turn`、`reconcile_plan_update`。
- `crates/agent/src/types.rs` — `ToolRuntimeEvent::PlanUpdate(UpdatePlanArgs)`。
- `crates/agent/src/persistence.rs` — `record_plan_cleared` / JSONL round-trip。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` — IM 卡片渲染。

## 用户目标验证清单

### 必须实现

- `update_plan` 工具接受 `explanation` + `plan[{step, status}]`,`status ∈ {pending, in_progress, completed}`,同时最多一个 `in_progress`,允许提交空 plan 作为清空快照。
- 每次 `update_plan` 成功后写回 `session.current_plan`,并推送到 `plan_sender`(IM 卡片、Web timeline)。
- 模型准备返回最终文本、且未继续调用工具时,若 `current_plan` 仍有未完成步骤,runtime 注入 system message 要求收口,并进入下一轮 loop;超过 2 次 `plan_repair_attempts` 后允许结束但记录 warning。
- 同一 turn 内 reconcile 已完成步骤,避免模型漏带或回退 completed 步骤。
- 上一 turn `current_plan` 全部 completed 时,下一条普通用户消息开始前清空,持久化 `plan_cleared` 事件。
- `TurnResult.plan_steps` 一律从 `session.current_plan` 取,IM 卡片与 API 返回基于同一份 plan 状态。

### 必须不破坏

- IM Gateway 现有单卡片 PATCH 刷新逻辑不变(首次 send 独立计划卡片,后续 PATCH 同一张)。
- 斜杠命令、状态查询等控制命令不触发新任务 plan 语义,不误清 completed plan。
- 模型自主结构化输出(工具、text delta、tool_calls_log)通道不受本方案影响。
- `record_plan_cleared` JSONL 事件对旧 session 文件向后兼容,遇到未知事件按现有 skip 策略处理。
- Agent Codex alignment / plan-mode / plan-lifecycle 三条既有设计文档的语义边界继续成立(本方案只做闭环加固)。

### 必须真实验证

- 单元:同一 turn 内两次 `update_plan`,第二次漏带第一次的 completed 步骤,reconcile 后仍保留;模型直接结束时 runtime 强制补一次收口;`plan_cleared` 持久化 round-trip。
- 接口 E2E:agent 会话触发 `update_plan`,最终 API 返回的 `plan_steps` 已收口;同一 `session_key` 下第二轮普通对话 `plan_steps` 不包含第一轮 completed 步骤。
- Human test:模型忘记收口时 runtime 补一次;IM 飞书卡片与 API 返回 plan 一致。

## 产品语义

### plan 是 runtime 会话状态

计划的“真相来源”是 `AgentSession.current_plan`,不是 tool_calls_log 中最后一次 `update_plan` 的解析结果。计划持久化通过 JSONL 事件:

- `plan_updated { plan }` — 每次 `update_plan` 成功。
- `plan_cleared { reason }` — 上一 turn 完全 completed 时下一 turn 清空。

恢复 session runtime state 时,按事件顺序回放,`plan_cleared` 把 `current_plan` 还原为 `None`,重置 `plan_repair_attempts`。

### 计划边界与用户 turn 对齐

- **同一 turn 内**:reconcile 已完成步骤,即使模型 `update_plan` 漏带或回退 completed 步骤,IM 卡片进度不倒退。
- **新任务边界**:上一 turn 计划全部 completed 时,runtime 在下一条**普通非斜杠**用户消息开始前清空 `current_plan`、重置 repair 计数、推送空 plan、持久化 `plan_cleared(reason = "new_turn_after_completion")`。
- **斜杠命令 / 状态查询**:不触发新任务语义,不清空 completed plan;这些命令不属于用户开始新任务。

### 强制收口

- 若 `session.current_plan.is_some_and(plan_has_unfinished_steps)` 且 `plan_repair_attempts < 2`,runtime 阻止本 turn 直接结束,注入一条 system message:
  > You already have an active task plan. Before concluding, call update_plan to reflect the final task state. If the work is complete, mark all steps as completed.
- `plan_repair_attempts.saturating_add(1)`,进入下一轮 loop 让模型补一次 `update_plan`。
- 超过上限后允许结束但记录 warning,避免异常模型死循环。

### 单卡片 PATCH

IM Gateway 展示层逻辑保持不变:第一次收到 plan 发送独立计划卡片,后续每次 `AgentTurnProgressEvent::PlanUpdated` 到达时 PATCH 同一张飞书卡片,不产生多张卡片。终态回复卡片中的“折叠计划面板”从 `TurnResult.plan_steps` 读,天然获得 runtime 闭环后的最终计划。

## 技术细节

### 工具定义

```rust
pub struct UpdatePlanArgs {
    pub explanation: Option<String>,
    pub plan: Vec<PlanStep>,
}

pub struct PlanStep {
    pub step: String,
    pub status: PlanStepStatus,
}

pub enum PlanStepStatus { Pending, InProgress, Completed }
```

工具校验:

- 最多一个 `in_progress`。
- 允许空 plan(清空快照)。
- 空 `step` 拒绝。
- `status` 字段不匹配枚举拒绝(严格 snake_case)。

工具输出:模型侧只返回 `"Plan updated"` 短确认,不再使用 `UPDATE_PLAN:{json}` 字符串;解析后的 `UpdatePlanArgs` 通过 `ToolResult.runtime_events = vec![ToolRuntimeEvent::PlanUpdate(args)]` 传给 turn loop。

### Session 层

```rust
pub struct AgentSession {
    // ...
    pub current_plan: Option<Vec<PlanStep>>,
    pub plan_repair_attempts: u8,
}
```

辅助函数:

- `plan_has_unfinished_steps(plan: &[PlanStep]) -> bool` — 存在 `pending` 或 `in_progress` 即 true。
- `plan_is_complete(plan: &[PlanStep]) -> bool` — 非空且不含未完成步骤。
- `reconcile_plan_update(current: &Option<Vec<PlanStep>>, incoming: Vec<PlanStep>) -> Vec<PlanStep>` — 同 turn 保留旧 completed 步骤。
- `clear_completed_plan_for_new_turn(session, recorder)` — 普通用户消息开始前调用,若上 turn 全 completed 则清空、reset repair、推空 plan、`record_plan_cleared`。

### Turn loop 闭环

`crates/agent/src/session/turn_loop.rs`:

1. **工具批次后立即写回**:
   ```rust
   ToolRuntimeEvent::PlanUpdate(args) => {
       let merged = reconcile_plan_update(&session.current_plan, args.plan.clone());
       session.current_plan = Some(merged.clone());
       session.plan_repair_attempts = 0;
       plan_sender.send(AgentTurnProgressEvent::PlanUpdated { steps: merged });
       recorder.record_plan_updated(&session.session_key, &args)?;
   }
   ```
2. **最终回答前强制收口**:
   ```rust
   if session.current_plan.as_ref().is_some_and(|p| plan_has_unfinished_steps(p))
       && session.plan_repair_attempts < 2 {
       session.plan_repair_attempts = session.plan_repair_attempts.saturating_add(1);
       inject_system_message(REPAIR_PROMPT);
       continue;  // 下一轮 loop
   }
   ```
3. **TurnResult 从 session 取**:`turn_result.plan_steps = session.current_plan.clone().unwrap_or_default();`
4. **新任务边界**:普通用户消息进入前 `clear_completed_plan_for_new_turn(session, &mut recorder)`。

### CLI / Admin API / Web

- **CLI**:`bifrost agent chat` / `bifrost agent run` 在终端 timeline 中展示 plan 更新,但不新增专门的 plan 子命令。plan 的“真相来源”仍在 session runtime。
- **Admin API**:
  - `POST /api/agent/session/:key/message` 返回 `TurnResult.plan_steps`。
  - `GET /api/agent/session/:key` 返回当前 `current_plan`(可选,视 admin 需要)。
  - 不新增 “PATCH plan” HTTP 端点——plan 语义由模型主导。
- **Web timeline**:
  - `web/src/pages/AI/AgentChatSection.timeline.ts` 消费 `AgentTurnProgressEvent::PlanUpdated`,渲染进度条 + 步骤列表。
  - `AgentChatSection.timeline.test.ts` 覆盖多次 plan 更新的时间线合并。

### Sync 边界

Plan 是每次 session 的运行时状态,不通过 rule/group sync 通道跨设备同步:

- session JSONL 存储在本地 `data_dir/agent/sessions/`,不上传云端。
- 组织/团队不能通过 sync 强制其他成员的 plan 状态。
- 团队协作靠 IM 卡片(飞书 group PATCH)在实时通道中共享,不走后端 sync。

## Phase 1 - 4

### Phase 1:runtime 状态化

- `AgentSession` 加 `current_plan` + `plan_repair_attempts`。
- `plan_has_unfinished_steps` / `reconcile_plan_update` / `clear_completed_plan_for_new_turn`。
- `ToolRuntimeEvent::PlanUpdate` 与 tool 返回值切换。
- 持久化 `plan_updated` / `plan_cleared` 事件 + 恢复回放。

### Phase 2:强制收口

- turn loop 最终回答前判定 unfinished + repair 计数。
- 注入 system message 提示模型补一次 `update_plan`。
- 上限 2 次后允许结束 + warning。

### Phase 3:展示层对齐

- IM Gateway `progress_card.rs` PATCH 同一张飞书卡片。
- Web timeline 消费 `PlanUpdated` 事件,合并同 turn 多次更新。
- `TurnResult.plan_steps` 统一来自 `session.current_plan`。

### Phase 4:human_tests / 文档

- `human_tests/update-plan.md` 新增“忘记收口 → runtime 强制补”“同 session 新任务不继承旧 completed”用例。
- README/skill 说明 plan lifecycle 与 IM 卡片行为。

## 测试方案

### 单元测试

`crates/agent/src/session/tests.rs`(全部真实存在):

- `plan_has_unfinished_steps` 三态覆盖。
- `test_clear_completed_plan_for_new_turn_resets_runtime_state`(line 300):`plan_repair_attempts` 被重置、runtime state 清空。
- 强制收口测试:`current_plan` unfinished + `plan_repair_attempts < 2` → 不结束;`>=2` → 允许结束 + warning。
- `reconcile_plan_update` 保留旧 completed 步骤;新任务边界不 reconcile 前 turn。
- `plan_cleared` JSONL 持久化 round-trip:写 → 读 → 恢复 session 后 `current_plan == None`。
- `AgentTurnProgressEvent::PlanUpdated` 推送包含 `steps` 与 `session_key`。

`crates/agent/src/tools/update_plan.rs`:

- 最多一个 `in_progress`。
- 空 plan 允许清空。
- 空 step 拒绝。
- 返回 `ToolRuntimeEvent::PlanUpdate(args)`,tool output 为 `"Plan updated"`。

### 接口化 E2E

`e2e-tests/tests/test_update_plan_human_api.sh`(已在仓库中):

1. 编译最新 Bifrost。
2. 临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
3. `POST /api/agent/session/:key/message` 提示模型创建 3 步计划并逐步完成。
4. 断言 `TurnResult.plan_steps` 全部 completed(收口成功)。
5. 同一 `session_key` 发第二条普通消息 → 断言 `plan_steps` 不包含第一轮 completed 步骤。
6. 断言 JSONL 中存在 `plan_updated` 与 `plan_cleared` 事件。

其它 e2e:`test_agent_chat_history_continue.sh`、`test_agent_codex_alignment_chat_api.sh`、`test_agent_run_timeline_channel_unification.sh`(全部真实存在)覆盖 plan 与 timeline / codex 对齐边界。

### 真实场景测试

`human_tests/update-plan.md` 补:

- TC-Plan-01:模型主动创建 3 步计划,逐步 update → 最终全部 completed。
- TC-Plan-02:模型直接说“完成”但 plan 还有 pending → runtime 强制补一次,IM 卡片最终显示全部 completed。
- TC-Plan-03:同 `session_key` 第二轮普通对话,IM 计划卡片重新开始新的步骤列表,不继承旧 completed。
- TC-Plan-04:斜杠命令(如 `/status`)不触发 completed plan 清空。
- TC-Plan-05:API `plan_steps` 与 IM 卡片 plan 一致。
- TC-Plan-06:超过 2 次 repair 后允许结束,session log 中包含 warning。

同时更新 `human_tests/agent-plan-lifecycle.md`、`human_tests/agent-codex-plan-mode.md`、`human_tests/external-runner-plan-ui.md`(如已存在)的边界描述,保持“plan 是 runtime 状态”的统一叙事。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标:runtime 状态化、强制收口、reconcile 保留 completed、新任务边界清空。
- Review 修改:`AgentSession` 字段、`turn_loop.rs` 闭环、`update_plan.rs` 校验与事件、persistence `plan_cleared`、IM progress_card。
- 跑 `cargo fmt --check`、`cargo clippy --workspace --all-features -D warnings`、`cargo test -p agent update_plan`、`cargo test -p agent session::tests`、`bash e2e-tests/tests/test_update_plan_human_api.sh`。

### 第 2 轮

- 复检:斜杠命令没有误清 plan、`plan_cleared` JSONL 事件在旧 session 文件上兼容、IM 卡片仍是单张 PATCH、`plan_repair_attempts` 不会在意外分支泄漏为 >2。
- 跑 `cargo test --workspace --all-features`、`pnpm --filter web test AgentChatSection.timeline`、执行 `human_tests/update-plan.md`。
- 观察 IM Gateway 真实群里的 plan 卡片是否只在同一张上 PATCH,没有出现多卡刷屏。

## 风险与决策

- **强制收口无限循环**:`plan_repair_attempts` 上限 2 次;超过后允许结束但打 warning,避免异常模型死锁在“再补一次 update_plan”提示上。
- **同 turn reconcile vs 新任务清空**:两个逻辑必须精确区分。reconcile 只在同一 turn iteration 内保留 completed;新任务清空只在“下一条普通用户消息”前触发。斜杠命令、状态查询、系统注入消息不触发新任务清空。
- **`UPDATE_PLAN:{json}` 遗留兼容**:旧模型可能仍产出这个前缀,现有实现把它按普通 tool output 处理,不再作为 plan 信号。如果需要迁移期兼容,可以在 tool wrapper 中把该前缀重新识别为 `PlanUpdate`,但推荐直接依赖新事件通道。
- **持久化 `plan_cleared` 事件兼容性**:老 session 文件没有该事件,恢复时按顺序回放天然兼容;JSONL reader 遇未知事件类型按 skip 处理,不阻塞恢复。
- **展示层不写 plan**:IM Gateway 与 Web timeline 只消费 `AgentTurnProgressEvent::PlanUpdated`,永远不写回 session。这保证 plan 只有一个真相来源。
- **模型侧 `Plan updated` 短确认**:如果模型对短确认敏感(例如 “内容太少怀疑失败”),需要提示或在 tool description 中显式说明返回值语义;实测多数模型不受影响。
