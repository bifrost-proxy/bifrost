# Update Plan 工具测试用例

## 功能模块说明

`update_plan` 是 Agent 内置工具，允许模型在执行复杂任务时结构化地记录和更新计划步骤（TODO/checklist）。每个步骤有 `pending` / `in_progress` / `completed` 三种状态。

**核心特性**：计划通过飞书卡片实时推送给用户。当 Agent 在 turn 执行过程中多次调用 `update_plan` 时，首次推送创建一张新的飞书卡片，后续调用通过 PATCH API 更新同一张卡片（而非每次新建），实现进度的实时刷新。最终回复卡片中也会包含计划面板。

## 前置条件

1. 启动 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=./.bifrost-test cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
   ```
2. 已配置 IM Gateway Provider（飞书机器人）并连接
3. Agent 已启用（`im/agent/config` API enabled=true）

## 测试用例

### TC-UP-01: update_plan 工具注册验证

**操作步骤**：
1. 调用 API 获取 Agent 工具列表：
   ```bash
   curl -s http://localhost:8800/_bifrost/api/im/agent/tools | jq '.tools[] | select(.function.name == "update_plan")'
   ```

**预期结果**：
- 返回 `update_plan` 工具定义
- 包含 `plan` (array, required) 和 `explanation` (string, optional) 参数
- description 包含 "TODO/checklist" 或 "task plan"

### TC-UP-02: Agent 对话触发 plan 实时推送

**操作步骤**：
1. 通过飞书向 Agent 发送一条需要多步骤的任务，例如：
   ```
   请帮我完成以下三个任务：1. 列出当前目录文件 2. 读取 README.md 3. 总结内容
   ```
2. 在 Agent 执行过程中观察飞书消息

**预期结果**：
- Agent 调用 `update_plan` 后，飞书中出现一张独立的计划卡片
- 卡片 header subtitle 显示 `📋 任务计划（X/Y）`
- 卡片 body 中每个步骤显示对应状态 emoji：⏳ pending、🔄 in_progress、✅ completed
- 当 Agent 再次调用 `update_plan` 时，同一张卡片被更新（PATCH），而非新建另一张卡片
- 最终回复卡片中也包含 `📋 任务计划（X/Y）` 折叠面板

### TC-UP-03: API 直接调用 Agent 验证 plan_steps 返回

**操作步骤**：
1. 通过 Agent chat API 发送消息，验证 plan_steps 出现在返回中：
   ```bash
   curl -s -X POST http://localhost:8800/_bifrost/api/im/agent/chat \
     -H "Content-Type: application/json" \
     -d '{"session_key":"test-plan","message":"请制定一个计划来：1. 查看当前目录 2. 查找所有 .md 文件 3. 总结发现"}' | jq .
   ```

**预期结果**：
- 返回 JSON 包含 `success: true`
- `response` 字段有内容
- `tool_calls` 数组可能包含 `update_plan` 调用记录

### TC-UP-04: 单元测试全部通过

**操作步骤**：
```bash
cargo test -p bifrost-agent -- update_plan
```

**预期结果**：
- 6 个测试全部通过：
  - `test_valid_arguments`
  - `test_with_explanation`
  - `test_invalid_status`
  - `test_multiple_in_progress_rejected`
  - `test_empty_plan_rejected`
  - `test_update_plan_signal`

### TC-UP-05: 编译检查无警告

**操作步骤**：
```bash
cargo clippy -p bifrost-agent -p bifrost-admin -- -D warnings
```

**预期结果**：
- 无 clippy 警告
- 编译成功

### TC-UP-06: plan_sender channel 集成验证

**操作步骤**：
1. 验证 `AgentSession` 结构体包含 `plan_sender` 字段：
   ```bash
   grep -n "plan_sender" crates/agent/src/session.rs
   ```
2. 验证 turn loop 中 UPDATE_PLAN 信号解析和 channel 推送逻辑：
   ```bash
   grep -n "UPDATE_PLAN\|plan_sender" crates/agent/src/session.rs
   ```

**预期结果**：
- `plan_sender` 字段定义为 `Option<tokio::sync::mpsc::UnboundedSender<Vec<PlanStep>>>`
- turn loop 中在检测到 `update_plan` 工具调用成功后，解析 `UPDATE_PLAN:{json}` 前缀并推送到 channel
- 推送失败时仅 debug 日志，不中断 turn

### TC-UP-07: patch_card API 集成验证

**操作步骤**：
1. 验证 `FeishuProvider` 包含 `patch_card` 方法：
   ```bash
   grep -n "patch_card" crates/bifrost-admin/src/im_gateway/feishu.rs
   ```

**预期结果**：
- `patch_card` 方法接受 `config`, `message_id`, `card` 参数
- 使用 PATCH `/im/v1/messages/{message_id}` API
- msg_type 为 `"interactive"`，content 为卡片 JSON 字符串

### TC-UP-08: plan listener spawn 验证（同一卡片更新机制）

**操作步骤**：
1. 验证 `im_gateway.rs` 中 plan listener spawn 逻辑：
   ```bash
   grep -n "plan_card_msg_id\|plan_rx\|plan_tx\|build_plan_card" crates/bifrost-admin/src/handlers/im_gateway.rs
   ```

**预期结果**：
- 在 `process_agent_chat` 中创建 `plan_tx`/`plan_rx` unbounded channel
- `plan_tx` 设置到 `session.plan_sender`
- spawn 异步任务监听 `plan_rx`
- 首次收到 steps 时通过 `send_card` 发送新卡片，保存 `message_id` 到 `plan_card_msg_id`
- 后续收到 steps 时通过 `patch_card` 更新同一张卡片（使用已保存的 `message_id`）
- `build_plan_card` 函数生成 Card 2.0 JSON，包含 `update_multi: true`

## 清理步骤

```bash
rm -rf ./.bifrost-test
```
