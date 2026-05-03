# update_plan 工具设计

## 功能概述

给 Agent 提供一个 `update_plan` 内置工具（类似 Codex 的 TodoWrite），让模型在执行复杂任务时可以结构化地记录和更新计划步骤，每个步骤有 `pending` / `in_progress` / `completed` 三种状态。计划通过飞书卡片实时展示进度给用户。

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
```

## 工具定义

- **名称**: `update_plan`
- **描述**: 更新任务计划（TODO/checklist），追踪多步骤任务的执行进度
- **参数 JSON Schema**:
  - `explanation` (string, optional): 当前计划说明
  - `plan` (array, required): 步骤列表
    - `step` (string, required): 步骤描述
    - `status` (string, required): `pending` / `in_progress` / `completed`

## 实现方案

### 1. 工具层（tools/update_plan.rs）

采用与 `set_title` 相同的"信号工具"模式：
- 工具本身只做参数验证，返回 `UPDATE_PLAN:{json}` 前缀的输出
- turn loop 解析前缀，提取 plan 数据存入 TurnResult

### 2. Turn 层（session.rs）

- `TurnResult` 新增 `plan_steps: Option<Vec<PlanStep>>` 字段
- turn loop 在每次工具批次执行后，检查是否有 `update_plan` 调用
- 取最后一次 `update_plan` 的结果作为最终 plan

### 3. 展示层（im_gateway.rs）

在飞书卡片中渲染 plan 步骤列表：
- 使用 Feishu Card 2.0 的 markdown 元素
- 每个步骤用 emoji 表示状态：⏳ pending, 🔄 in_progress, ✅ completed
- 放在回复内容下方、工具调用记录上方

## 测试方案

- 单元测试：验证 update_plan 工具参数解析（空 plan、非法 status 等）
- 真实场景测试：`human_tests/update-plan.md`
