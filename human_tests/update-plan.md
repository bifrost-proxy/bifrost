# Update Plan 工具测试用例

## 功能模块说明

本模块验证 Agent `update_plan` 从“本轮局部信号”升级为“runtime 持有的 session 级状态”后的真实行为，重点覆盖：

1. `/_bifrost/api/im/agent/chat` 最终响应会返回 `plan_steps`
2. 当模型尝试在计划未收口时直接结束回答，runtime 会强制插入补救提示，要求再次调用 `update_plan`
3. 计划收口后，最终返回的 `plan_steps` 全部为 `completed`
4. 同一 turn 内后续 `update_plan` 是当前快照替换，不会继承本 turn 早先已完成但已删除的步骤
5. 模型提交 `plan: []` 时，runtime 会清空当前 plan，而不是要求模型编造一个占位步骤

本次真实场景测试以**真实 Bifrost 进程 + 真实 Admin API + 本地 mock model server** 方式执行，禁止仅用 grep / 静态检查代替。

## 前置条件

1. 当前目录位于仓库根目录
2. 本地已具备 Rust 构建环境
3. 测试端口避开正式环境 `9900`，统一使用临时端口
4. 启动 Bifrost 时必须带 `--no-system-proxy`

## 测试用例列表

### TC-UP-01：工具注册接口暴露 update_plan

**操作步骤**：
1. 构建二进制：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   ```
2. 使用临时数据目录启动 Bifrost：
   ```bash
   TEST_DIR=$(mktemp -d)
   BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
     --host 127.0.0.1 \
     -p 18881 \
     --unsafe-ssl \
     --skip-cert-check \
     --no-system-proxy
   ```
3. 在另一个 shell 调用工具列表接口：
   ```bash
   curl -fsS --noproxy '*' http://127.0.0.1:18881/_bifrost/api/im-gateway/agent/tools | jq '.tools[] | select(.function.name == "update_plan")'
   ```
4. 停止步骤 2 启动的 Bifrost 进程，并删除临时目录。

**预期结果**：
- 接口返回 `update_plan` 工具定义
- 工具参数中包含 `plan`
- 工具描述中包含 TODO/checklist 或 task plan 语义

### TC-UP-02：runtime 在真实 API 对话中强制收口未完成计划，并隔离下一轮新任务计划

**操作步骤**：
1. 运行黑盒 API 回归脚本：
   ```bash
   e2e-tests/tests/test_update_plan_human_api.sh
   ```
2. 该脚本内部会完成以下动作：
   - 启动本地 mock model server
   - `cargo build --bin bifrost`
   - 用临时数据目录启动真实 Bifrost 进程
   - PATCH `/_bifrost/api/im-gateway/agent` 指向 mock provider
   - 调用 `POST /_bifrost/api/im-gateway/agent/chat`
   - 校验 mock 模型先提交未完成计划、随后试图直接结束、再被 runtime 强制要求补一次 `update_plan`
   - 校验 mock 模型随后提交一个更小的最终快照，最终 `plan_steps` 严格等于这个快照
   - 使用同一 `session_key` 再发起第二轮真实 API 对话
   - 校验第二轮新任务的 `plan_steps` 只包含新计划，不继承第一轮已完成步骤
   - 使用同一 `session_key` 再发起第三轮真实 API 对话，mock 模型提交 `update_plan {"plan":[]}`
   - 校验第三轮响应的 `plan_steps` 为 `null`，表示当前快照已清空

**预期结果**：
- 脚本输出 `PASS`
- `/_bifrost/api/im-gateway/agent/chat` 最终响应包含 `plan_steps`
- `plan_steps` 中所有步骤状态均为 `completed`
- 第一轮最终 `plan_steps` 严格等于 `[{"step":"Deliver concise answer","status":"completed"}]`
- 第一轮最终 `plan_steps` 不包含早先计划中的 `Inspect workspace` / `Summarize findings`
- `tool_calls` 中至少出现三次 `update_plan`
- 可以证明真正生效的是 runtime gate，而不是模型第一次就碰巧收口
- 第二轮响应的 `plan_steps` 严格等于 `[{"step":"Handle follow-up question","status":"completed"}]`
- 第二轮响应不包含第一轮的 `Inspect workspace` / `Summarize findings` 旧步骤
- 第三轮响应的 `plan_steps` 为 `null`
- 第三轮 `tool_calls` 中的 `update_plan` 参数包含空数组 `plan: []`

### TC-UP-03：Agent 侧 helper 回归测试通过

**操作步骤**：
1. 执行本次新增/修改的 helper 单元测试：
   ```bash
   cargo test -p bifrost-agent --all-features plan_
   ```

**预期结果**：
- `update_plan` 成功工具结果为 `Plan updated`，不再包含 `UPDATE_PLAN:` 内部 JSON 信号
- `ToolResult.runtime_events` 能携带 typed `PlanUpdate`，turn loop 不再从工具输出文本反解析 plan
- `apply_plan_update_snapshot` 会用 incoming plan 替换历史 plan，允许旧 completed 步骤从当前快照中消失或重新打开
- `plan_has_unfinished_steps` 能正确识别 `pending` / `in_progress`
- `clear_completed_plan_for_new_turn` 会重置 `current_plan` 与 `plan_repair_attempts`，并向展示通道发送空计划
- `apply_plan_update_snapshot` 接收空 plan 时会清空 `current_plan`，持久化 `plan_cleared`，并向展示通道发送空计划
- `plan_cleared` 持久化事件在恢复 runtime state 时会清空历史计划

## 清理步骤

1. 删除测试过程中创建的临时目录
2. 确认没有残留的 mock model server 或 bifrost 进程
3. 如需再次执行，可直接重新运行上述命令
