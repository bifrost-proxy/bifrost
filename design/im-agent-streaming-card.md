# IM Agent Streaming Progress Card

## 功能模块说明

IM Gateway Agent 在收到来自 IM 的消息后，需要用同一张进度卡片持续展示 Agent loop 的执行状态，直到 loop 结束。卡片在执行期间保持流式更新；结束时写入最终输出并关闭流式模式。

本设计的核心约束是“一个 IM Agent turn 对应一张进度卡片”。收到 IM 消息后立即创建并发送 CardKit streaming card；Agent loop 生命周期内只更新这张卡片的固定组件。guide/queue 变化进入底部状态栏并触发同卡刷新，loop 结束时最后 flush 最终输出，再关闭 `streaming_mode=false`。

## 目标

- 来自 IM 的 Agent loop 使用单张进度卡片承载执行状态。
- 卡片包含四类用户可见信息：
  - 最终输出：执行中显示 `处理中...`，结束后直接显示最终回复，不额外渲染“最终输出”标题。
  - TodoList：仅当 Agent 调用 `update_plan` 后展示当前计划；没有计划时不渲染该模块；折叠标题展示当前正在处理的任务。
  - 工具执行状态：仅当出现工具事件后渲染；默认折叠详情，折叠外展示最新工具名称和基本状态。
  - 底部状态信息：默认折叠，折叠标题只展示 token 消耗，展开后展示 loop 状态、context 用量、压缩次数、排队消息、guide 状态、工作路径。
- 过程思考信息独立进入底部折叠面板，折叠标题展示一行摘要，展开后展示最后一次正在输出的完整过程文本，不混入最终输出区域。
- 最终输出模块始终放在卡片最后，用过程模块先展示执行进展，再用最终回复收束。
- guide 消息进入时更新同一张卡片的底部 guide 状态。
- queue 消息进入或删除时更新同一张卡片的底部排队状态。
- 架构上不把能力写死为 Feishu 私有逻辑；IM Gateway 提供 provider-neutral progress snapshot / renderer / capability 入口，Feishu 是第一版实现。

## 非目标

- 第一版不实现卡片内交互按钮。Feishu streaming mode 下处理卡片回调需要先关闭流式模式，会扩大状态机复杂度。
- 第一版不承诺模型 token delta 真流式。若模型客户端尚未暴露 delta，最终输出在 loop 结束时一次性写入；计划、工具、状态仍持续更新。
- 第一版不为所有 IM provider 实现原地更新。provider 能力由 capability 描述，后续接入者按能力降级。

## 架构设计

### Provider-neutral progress snapshot

Agent runtime 只产生与 IM 平台无关的事件：

- `Status`
- `ToolStarted`
- `ToolFinished`
- `PlanUpdated`
- `TitleUpdated`
- `AssistantDelta`
- `AssistantFinal`
- `TurnFinished`
- `TurnFailed`

IM Gateway 把这些事件归并为 `ImAgentProgressSnapshot`。snapshot 是后续所有 IM renderer 的共同输入，包含：

- `session_key`
- `title`
- `output`
- `last_thought`
- `plan_steps`
- `tool_calls`
- `latest_tool`
- `status`
- `queue_items`
- `guide_pending`
- `phase`

### Provider capability

后续 IM provider 接入时至少声明三类能力之一：

- `StreamingCard`：支持创建流式卡片、组件内容更新、关闭流式模式。Feishu CardKit 属于此类。
- `PatchMessage`：支持发送消息后原地更新消息，但不支持真正 streaming。
- `SendOnly`：只支持发送新消息。该模式仍可复用 snapshot renderer，但无法满足同卡持续更新。

Feishu V1 使用 `StreamingCard`，通过 CardKit 创建 card entity，再用 IM send API 发送 card entity。执行中更新固定元素内容和工具折叠面板，结束时关闭 `streaming_mode`。guide/queue 触发时只更新同一卡片 footer；如果找不到活跃 progress session，才回退发送普通确认消息。

### Feishu CardKit lifecycle

```text
IM message received
  -> create CardKit card entity with streaming_mode=true
  -> send interactive message with card_id
  -> Agent loop emits progress events
  -> coalesce progress events
  -> update element content: output / optional plan / optional tool panel / folded status / optional thought
  -> guide or queue update
       -> update same card folded status with guide/queue state
  -> loop finished
       -> final output update
       -> close streaming_mode=false with final summary
```

## 实现逻辑

- `crates/agent` 新增 progress event 通道，挂在 `AgentSession` 上。
- `refresh_active_turn_status` 自动向 progress 通道发送 status snapshot。
- 工具调用开始、工具调用结束、计划更新、标题更新、过程文本和最终回复产生对应 progress event。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` 维护 IM progress snapshot 和 Feishu streaming card session。
- `run_agent_chat_with_interleave` / `process_agent_chat` 创建 progress session，并把 progress sender 注入 AgentSession。
- `run_progress_event_coalescer` 对 status 类事件按 300ms 合并刷新，工具、计划、标题、过程文本、最终输出和结束事件立即刷新；Feishu session 继续按 section fingerprint 过滤未变化模块，避免 status-only 更新打出多次无效 CardKit API。
- `handle_busy_message` 在 guide / queue / remove queue 成功后通知 progress session 更新同一张卡片折叠状态区，并在状态区标题里加入一条轻量可见提示；如果没有活跃卡片或刷新失败，再回退发送普通确认卡片。
- 当 `set_title` 工具刷新标题时，通过 CardKit 整卡更新刷新 header；如果没有工具标题，则初始标题使用用户消息。
- CardKit 更新 uuid 使用短随机值，不拼接 `card_id`；loop 结束时即使最终内容 flush 失败，也会 best-effort 关闭 `streaming_mode=false`。
- progress outbound message log 记录真实 Feishu `message_id`，并把 CardKit `card_id` 写入 target 线索，便于排查具体卡片更新链路。

## 测试方案

### 单元测试

- progress snapshot 应能从 status / plan / tool / final output 事件归并出稳定卡片内容。
- guide/queue 状态进入 snapshot 后，footer 显示排队数量和 guide pending 状态。
- Feishu streaming card JSON 包含 JSON 2.0、`streaming_mode=true`、固定 element_id。
- 无计划、无工具、无过程文本时，不渲染对应模块。
- 工具执行状态使用默认折叠的 `agent_tool_panel`，折叠标题展示最新工具名、成功/失败、耗时和累计次数。
- 状态区使用默认折叠的 `agent_status_panel`，折叠标题只展示 token 消耗。
- guide/queue 注入后，状态区标题追加“已收到引导 / 已加入排队 / 已删除排队”的轻量提示，避免用户误以为输入没有反馈。
- 计划面板使用 `agent_plan_panel` 组件级更新标题和内容，标题优先展示 in-progress step。
- 过程文本使用默认折叠的 `agent_thinking_panel`，标题展示最后一次 `AssistantDelta` 的一行摘要，展开后展示完整内容。
- guide/queue 更新不创建新 card entity，不撤回旧消息，只刷新同一 card 的折叠状态区。
- CardKit 更新 uuid 长度保持在 Feishu 字段限制内；final flush 失败时仍尝试关闭 streaming。

### E2E 测试

- 新增 IM Agent streaming card 回归，验证 progress renderer 输出 JSON 2.0 streaming card、固定组件 id、可选 plan/tool/thinking 模块、工具耗时和 guide/queue 状态区。
- 对真实 Agent loop 路径执行最小模型 mock，确认工具调用、计划更新、最终输出能进入 progress event。

### 真实场景测试

- 更新 `human_tests/im-gateway-agent.md`，新增流式进度卡片用例：
  - 正常 Agent loop：卡片持续更新，结束后关闭 streaming，标题跟随 `set_title`。
  - guide 消息：同一卡片折叠状态区展示待处理 guide，不撤回不重发。
  - queue 消息：同一卡片折叠状态区展示排队数量，不撤回不重发。
- 更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：确认四区卡片、guide/queue 同卡刷新、provider-neutral 入口都已落地。
- 代码 review：重点检查 Feishu API request 序列、进度 task 生命周期、并发锁、失败降级、旧普通卡片路径兼容。
- 测试：运行 progress/feishu/card/queue 相关单元测试与新增 E2E。

### 第 2 轮

- 再次复核 diff、docs、human_tests 索引和真实执行记录。
- 复跑第一轮失败或修复过的路径。
- 若发现 card lifecycle、queue/guide 或 status 同步缺口，继续追加轮次。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 受影响 crate 的单元测试。
- 新增/更新 E2E。
- `cargo test --workspace --all-features`
- 按修改范围评估 `scripts/ci/local-ci.sh`。

## 文档更新要求

- 更新 `human_tests/im-gateway-agent.md`。
- 更新 `human_tests/readme.md`。
- 如 provider capability 后续暴露到 WebUI 或 CLI，再同步 README；本次仅内部架构和 IM 行为变化，不新增用户可配置命令。
