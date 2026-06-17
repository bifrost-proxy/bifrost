# update_plan 工具设计

## 功能概述

给 Agent 提供一个 `update_plan` 内置工具，让模型在执行复杂任务时可以结构化地记录和更新计划步骤，每个步骤有 `pending` / `in_progress` / `completed` 三种状态。计划通过飞书卡片实时展示进度给用户。

本次修复的核心目标不是“看起来像有计划”，而是把 `update_plan` 从**单次 turn 的展示信号**升级为 **runtime 持有的会话状态**：

1. **计划语义仍由模型负责**：步骤内容、状态迁移仍由模型通过 `update_plan` 明确表达。
2. **计划生命周期由 runtime 强制闭环**：一旦会话中存在 active plan，模型在最终回答前必须把计划收口到最终状态。
3. **展示层只渲染 runtime 状态**：IM 卡片和 API 返回都基于 `AgentSession.current_plan`，不再依赖当前 turn 的临时提取结果。
4. **计划边界与用户 turn 对齐**：同一 turn 内保留已完成步骤，避免模型回退；已完成计划进入下一条普通用户消息时清空，避免旧任务 completed 步骤污染新任务。

plan 是真实运行态，而不是 prompt + UI 拼接出来的附属物。

## 数据结构

```rust
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

pub struct PlanStep {
    pub step: String,
    pub status: PlanStepStatus,
}

pub struct UpdatePlanArgs {
    pub explanation: Option<String>,
    pub plan: Vec<PlanStep>,
}

pub struct AgentSession {
    // ...
    pub current_plan: Option<Vec<PlanStep>>,
    pub plan_repair_attempts: u8,
}
```

## 工具定义

- **名称**: `update_plan`
- **描述**: 更新任务计划（TODO/checklist），追踪多步骤任务的执行进度
- **参数 JSON Schema**:
  - `explanation` (string, optional): 当前计划说明
  - `plan` (array, required): 步骤列表
    - `step` (string, required): 步骤描述
    - `status` (string, required): `pending` / `in_progress` / `completed`

## 旧实现问题

旧实现虽然提供了 `update_plan` 工具，但本质上仍然是“信号工具”：

1. 工具返回 `UPDATE_PLAN:{json}` 字符串。
2. turn loop 在工具批次执行后从 `tool_calls_log` 里扫描最后一次成功调用。
3. 把该结果临时放进 `TurnResult.plan_steps`。
4. IM Gateway 再用这个临时结果渲染卡片。

这会导致两个关键问题：

- **计划不是 session 级状态**：跨 iteration 只有临时局部变量保存最后一次 plan。
- **最终回答前没有强制收口**：如果模型已经完成任务但忘记再调一次 `update_plan`，最终 plan 仍可能停留在 `pending/in_progress`。

## 实现方案

### 1. 工具层（tools/update_plan.rs）

`update_plan` 接口和参数（`explanation` + `plan`）保持不变。当前实现已不再使用 `UPDATE_PLAN:{json}` 字符串信号，而是把解析后的 `UpdatePlanArgs` 通过 `ToolResult.runtime_events` 里的 `ToolRuntimeEvent::PlanUpdate(args)` 类型化事件返回给 turn loop；模型侧 tool output 仅返回 `"Plan updated"` 短确认。工具同时校验“最多一个 in_progress 步骤”并允许提交空 plan 作为清空快照。

### 2. Session 层（session.rs）

新增 session 级 plan 状态：

- `current_plan: Option<Vec<PlanStep>>`
  - 会话当前真实计划状态。
  - 每次 `update_plan` 成功后立即写回。
- `plan_repair_attempts: u8`
  - runtime 在“最终回答前强制收口”时的有限重试计数。
  - 防止模型异常时无限循环。

计划持久化新增 `plan_cleared` 事件：当已完成计划在下一条普通用户消息开始前被清空时写入 JSONL，恢复 session runtime state 时该事件会把 `current_plan` 还原为 `None`。

同时增加两个辅助逻辑：

- `plan_has_unfinished_steps(plan)`
  - 只要存在 `pending` 或 `in_progress`，就认为还未收口。
- `extract_plan_update(tool_calls_log)`
  - 从最新成功的 `update_plan` 中解析 `UpdatePlanArgs.plan`（现以 `ToolRuntimeEvent::PlanUpdate` 形式从 `ToolResult.runtime_events` 直接获取，不再扫描 `UPDATE_PLAN:` 文本）。
  - 解析成功后更新 `session.current_plan`，并重置 `plan_repair_attempts`。
- `clear_completed_plan_for_new_turn(session)`
  - 只在普通非斜杠用户消息开始前运行。
  - 若上一轮计划已经全部 `completed`，清空 `current_plan`、重置 repair 计数、推送空 plan 到 runtime 展示通道，并持久化 `plan_cleared`。
- `reconcile_plan_update(current, incoming)`
  - 同一 turn 内保留旧的 completed 步骤，防止同一任务中模型漏带或回退已完成步骤。
  - 新任务边界通过 turn 开始时清空 completed plan 实现隔离，避免旧 completed 步骤进入新任务 reconcile 输入。

### 3. Turn loop 闭环策略

#### 3.1 工具批次后立即写回 session plan

在每轮工具执行完成后：

- 从最新 `update_plan` 成功调用中解析出 `Vec<PlanStep>`
- 写入 `session.current_plan`
- 推送到 `plan_sender`，让 IM 卡片实时刷新
- 在同一 turn 内 reconcile 已完成步骤：如果模型后续 `update_plan` 漏带或回退已完成步骤，runtime 会保留这些 completed 步骤，避免卡片进度倒退。

这样 plan 的真相来源变成 `session.current_plan`，而不是局部变量。

#### 3.2 最终回答前强制 plan 收口

在模型准备返回最终文本、且没有继续调用工具时：

- 如果 `session.current_plan` 不存在，则按普通流程结束。
- 如果 `session.current_plan` 存在且仍有未完成步骤：
  - runtime 不直接结束 turn
  - 向 history 注入一条 system message，明确要求模型先调用 `update_plan` 收口
  - 继续下一轮 loop

注入提示示例：

> You already have an active task plan. Before concluding, call update_plan to reflect the final task state. If the work is complete, mark all steps as completed.

为了避免死循环，`plan_repair_attempts` 设置有限上限（例如 2 次）。如果模型连续忽略，runtime 记录 warning 后允许本轮结束，但正常模型路径下应在 repair round 完成收口。

#### 3.3 TurnResult 从 session 状态取 plan

最终返回 `TurnResult` 时：

- `plan_steps` 一律取 `session.current_plan.clone()`
- 不再依赖本 turn 的临时 `last_plan_steps`

这保证：
- API 返回与 IM 卡片展示基于同一份 plan 状态
- 多轮 iteration 下不会因为局部变量遗漏导致状态丢失

#### 3.4 已完成计划的新 turn 清理

当用户在同一 `session_key` 中开启下一条普通消息时，如果上一轮 `current_plan` 已全部 completed：

- runtime 在模型调用前清空 `current_plan`
- 记录 `plan_cleared(reason = "new_turn_after_completion")`
- 向进度/计划通道推送空 plan，让展示层不再沿用旧任务状态

该清理只发生在普通用户消息；斜杠命令、状态查询等控制命令不会触发新任务 plan 语义。

### 4. 展示层（im_gateway.rs）

展示层逻辑保持不变，继续通过 `plan_sender` 监听 plan 更新并刷新同一张飞书卡片：

- 首次收到 plan：发送独立计划卡片
- 后续收到 plan：PATCH 同一张卡片
- 最终回复卡片中的折叠计划面板也使用 `TurnResult.plan_steps`

由于 `TurnResult.plan_steps` 已切换为 session 状态，展示层天然获得 runtime 闭环后的最终计划。

## 行为预期

修复后，复杂任务的典型行为变为：

1. 模型创建 plan。
2. 模型执行若干工具调用并多次更新 plan。
3. 模型尝试直接结束，但 plan 仍含 `pending/in_progress`。
4. runtime 阻止直接结束，要求模型补一次 `update_plan`。
5. 模型把计划收口为最终状态（通常全部 `completed`）。
6. 最终答复与 IM 卡片、API 返回中的 `plan_steps` 一致。
7. 用户下一条普通消息开始新任务时，旧 completed 计划被 runtime 清空；新任务第一次 `update_plan` 只展示新步骤。

## 测试方案

### 单元测试

新增/补充以下单元测试：

- `plan_has_unfinished_steps`：验证 pending / in_progress / all completed 三种情况
- `extract_plan_update`：验证可从 `ToolRuntimeEvent::PlanUpdate` 提取计划
- `plan repair gate`：验证已有 active plan 且未完成时，不会直接结束而会进入 repair 分支
- `clear_completed_plan_for_new_turn`：验证 completed plan 在下一条普通消息前清空、重置 repair 计数并发送空 plan 进度事件
- `reconcile_plan_update`：验证同一 turn 内会保留旧 completed 步骤，且不会回退已完成状态
- `plan_cleared` 持久化 round trip：验证恢复 runtime state 时 `plan_cleared` 会清空历史 plan

### 接口化 E2E 测试

必须按用户要求使用“先编译、再启动 Bifrost、再通过真实接口验证”的方式：

1. 编译最新 Bifrost 二进制
2. 使用临时数据目录启动服务
3. 调用 Agent 对话接口触发 `update_plan`
4. 验证最终 API 返回的 `plan_steps` 已收口为预期状态
5. 使用同一 `session_key` 发起第二轮普通对话，验证新任务 `plan_steps` 不包含第一轮已完成步骤

### 真实场景测试

更新 `human_tests/update-plan.md`：

- 补充“任务完成但模型先忘记收口时，runtime 会强制补收口”用例
- 补充“最终 API plan_steps 与实时 plan card 一致”用例
- 补充“同一 session 下一轮新任务不会继承上一轮 completed 计划”用例
- 按文档逐条真实执行
