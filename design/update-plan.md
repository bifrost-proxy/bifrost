# update_plan 工具设计

## 功能概述

给 Agent 提供一个 `update_plan` 内置工具（类似 Codex 的 TodoWrite），让模型在执行复杂任务时可以结构化地记录和更新计划步骤，每个步骤有 `pending` / `in_progress` / `completed` 三种状态。计划通过飞书卡片实时展示进度给用户。

本次修复的核心目标不是“看起来像有计划”，而是把 `update_plan` 从**单次 turn 的展示信号**升级为 **runtime 持有的会话状态**：

1. **计划语义仍由模型负责**：步骤内容、状态迁移仍由模型通过 `update_plan` 明确表达。
2. **计划生命周期由 runtime 强制闭环**：一旦会话中存在 active plan，模型在最终回答前必须把计划收口到最终状态。
3. **展示层只渲染 runtime 状态**：IM 卡片和 API 返回都基于 `AgentSession.current_plan`，不再依赖当前 turn 的临时提取结果。

这更接近 Codex 的设计哲学：plan 是真实运行态，而不是 prompt + UI 拼接出来的附属物。

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

保持现有 `update_plan` 接口和参数不变，继续使用 `UPDATE_PLAN:{json}` 作为最小侵入实现。

原因：
- 保持模型侧调用方式稳定。
- 本轮优先修复 runtime 生命周期问题，而不是同时重构所有 tool side effect 通道。

后续可再把字符串信号升级为结构化 side effect。

### 2. Session 层（session.rs）

新增 session 级 plan 状态：

- `current_plan: Option<Vec<PlanStep>>`
  - 会话当前真实计划状态。
  - 每次 `update_plan` 成功后立即写回。
- `plan_repair_attempts: u8`
  - runtime 在“最终回答前强制收口”时的有限重试计数。
  - 防止模型异常时无限循环。

同时增加两个辅助逻辑：

- `plan_has_unfinished_steps(plan)`
  - 只要存在 `pending` 或 `in_progress`，就认为还未收口。
- `extract_plan_update(tool_calls_log)`
  - 从最新成功的 `update_plan` 中解析 `UpdatePlanArgs.plan`。
  - 解析成功后更新 `session.current_plan`，并重置 `plan_repair_attempts`。

### 3. Turn loop 闭环策略

#### 3.1 工具批次后立即写回 session plan

在每轮工具执行完成后：

- 从最新 `update_plan` 成功调用中解析出 `Vec<PlanStep>`
- 写入 `session.current_plan`
- 推送到 `plan_sender`，让 IM 卡片实时刷新

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

## 测试方案

### 单元测试

新增/补充以下单元测试：

- `plan_has_unfinished_steps`：验证 pending / in_progress / all completed 三种情况
- `extract_plan_update`：验证可从 `UPDATE_PLAN:{json}` 提取计划
- `plan repair gate`：验证已有 active plan 且未完成时，不会直接结束而会进入 repair 分支

### 接口化 E2E 测试

必须按用户要求使用“先编译、再启动 Bifrost、再通过真实接口验证”的方式：

1. 编译最新 Bifrost 二进制
2. 使用临时数据目录启动服务
3. 调用 Agent 对话接口触发 `update_plan`
4. 验证最终 API 返回的 `plan_steps` 已收口为预期状态

### 真实场景测试

更新 `human_tests/update-plan.md`：

- 补充“任务完成但模型先忘记收口时，runtime 会强制补收口”用例
- 补充“最终 API plan_steps 与实时 plan card 一致”用例
- 按文档逐条真实执行
