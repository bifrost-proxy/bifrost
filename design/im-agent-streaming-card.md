# IM Agent Streaming Progress Card

## 功能模块说明

IM Gateway Agent 在收到来自 IM 的消息后，需要用同一张进度卡片持续展示 Agent loop 的执行状态，直到 loop 结束。卡片在执行期间保持流式更新；结束时写入最终输出并关闭流式模式。

本设计的核心约束是“一个 IM Agent 可见执行阶段对应一张进度卡片”。收到 IM 消息后立即创建并发送 CardKit streaming card；Agent loop 生命周期内更新这张卡片的固定组件。运行中收到 guide/queue 用户消息时，旧的 Running 卡片会 best-effort 更新为结束/冻结状态并关闭 `streaming_mode`，新发一张继承当前 snapshot 的 CardKit 卡片到最新用户消息下方，后续进展只更新新卡片。这样消息流会保留“用户插话时的执行快照”，同时最新进度仍跟随最新用户消息。loop 真正结束后卡片进入 Finished/Failed 终态，终态历史卡片必须保留，后续独立新消息只能新建下一张卡片，不能改写或撤回已完成卡片。

## 目标

- 来自 IM 的 Agent loop 使用单张进度卡片承载执行状态。
- 卡片包含四类用户可见信息：
  - 最终输出：执行中显示 `处理中...`，结束后直接显示最终回复，不额外渲染“最终输出”标题。
  - TodoList：仅当 Agent 调用 `update_plan` 后展示当前计划；没有计划时不渲染该模块；折叠标题展示当前正在处理的任务。
  - 工具执行状态：仅当出现工具事件后渲染；默认折叠详情，折叠外展示最新工具名称和基本状态。
  - 底部状态信息：默认折叠，折叠标题只展示 token 消耗，展开后展示 loop 状态、context 用量、压缩次数、排队消息、guide 状态、工作路径。
- 过程思考信息独立进入“思考过程”模块，默认直接展示最后一次正在输出的完整过程文本，不需要展开操作，也不混入最终输出区域。
- 最终输出模块始终放在卡片最后，用过程模块先展示执行进展，再用最终回复收束。
- guide 消息进入时更新 Running 卡片的底部 guide 状态，冻结旧卡片并新发一张当前快照卡片到最新用户消息之后。
- queue 消息进入或删除时更新 Running 卡片的底部排队状态，冻结旧卡片并新发一张当前快照卡片到最新用户消息之后；终态历史卡片不参与 rollover。
- 架构上不把能力写死为 Feishu 私有逻辑；IM Gateway 提供 provider-neutral progress snapshot / renderer / capability 入口，Feishu 是第一版实现。

## 非目标

- 第一版不实现卡片内交互按钮。Feishu streaming mode 下处理卡片回调需要先关闭流式模式，会扩大状态机复杂度。
- 第一版不承诺模型 token delta 真流式。若模型客户端尚未暴露 delta，最终输出在 loop 结束时一次性写入；计划、工具、状态仍持续更新。
- 第一版不为所有 IM provider 实现原地更新。provider 能力由 capability 描述，后续接入者按能力降级。

## 架构设计

### Provider-neutral progress snapshot

Agent runtime 只产生与 IM 平台无关的事件：
- `Status`
- `ContextUpdated`
- `CompactionStarted` / `CompactionFinished` / `CompactionFailed`
- `ToolStarted`
- `ToolFinished`
- `LongTaskStatus`
- `PlanUpdated`
- `ProposedPlan`
- `TitleUpdated`
- `AssistantDelta`
- `AssistantFinal`
- `TurnFinished`
- `TurnFailed`
- `TurnFailed`

IM Gateway 把这些事件归并为 `ImAgentProgressSnapshot`。snapshot 是后续所有 IM renderer 的共同输入，包含：
- `session_key`
- `title`
- `output`
- `last_thought`
- `runner`（adapter / model / token usage / work_dir 等 runner 摘要）
- `plan_steps`
- `proposed_plan`
- `tool_calls`
- `latest_tool`
- `timeline`（thinking / tool / status 时间线）
- `status`
- `context`（context 用量与压缩计数）
- `queue_items`
- `guide_pending`
- `activity_notice`
- `phase`
- `phase`

### Provider capability

后续 IM provider 接入时至少声明三类能力之一：

- `StreamingCard`：支持创建流式卡片、组件内容更新、关闭流式模式。Feishu CardKit 属于此类。
- `PatchMessage`：支持发送消息后原地更新消息，但不支持真正 streaming。
- `SendOnly`：只支持发送新消息。该模式仍可复用 snapshot renderer，但无法满足同卡持续更新。

Feishu V1 使用 `StreamingCard`，通过 CardKit 创建 card entity，再用 IM send API 发送 card entity。执行中更新固定元素内容和工具折叠面板，结束时关闭 `streaming_mode`。guide/queue 触发时更新 progress snapshot，先发送一张继承当前 snapshot 的新卡片，再 best-effort 把旧卡片整卡更新为冻结快照并关闭 streaming；如果找不到活跃 progress session，才回退发送普通确认消息。

### Feishu CardKit lifecycle

```text
IM message received
  -> create CardKit card entity with streaming_mode=true
  -> send interactive message with card_id
  -> Agent loop emits progress events
  -> coalesce progress events
  -> update element content: output / optional plan / optional tool panel / optional process timeline (`agent_process_panel`) / folded status / visible thought
  -> guide or queue update
       -> create and send a new CardKit card entity below the latest user message
       -> best-effort update previous card entity as a frozen Finished snapshot
       -> best-effort close previous card streaming_mode=false
  -> CardKit update returns code=300305 element exceeds the limit
       -> create and send a new CardKit card entity with the latest snapshot
       -> if the new full snapshot card also returns code=300305, retry once with a compact status card
       -> subsequent progress and final updates target the new card_id
       -> best-effort freeze/close the oversized previous card
  -> loop finished
       -> final output update
       -> close streaming_mode=false with final summary
  -> queued message becomes next turn
       -> if previous card is still Running, create a new card and freeze/close previous card
       -> if previous card is Finished/Failed, keep it in history and create a new card directly
       -> send a new CardKit card entity below the latest user message
       -> subsequent progress events update the new card_id
  -> send card entity returns `cardid is invalid`
       -> treat the just-created CardKit entity as unusable
       -> create a fresh CardKit card entity and retry sending once
       -> if retry succeeds, subsequent progress and final updates target the fresh card_id
       -> if retry still fails, do not emit a synthetic "started runner task" IM message; only send the final fallback reply when the run finishes
```

## 实现逻辑

- `crates/agent` 新增 progress event 通道，挂在 `AgentSession` 上。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` 维护 IM progress snapshot 和 Feishu streaming card session；除了基础 status/plan/tool/final 事件外，snapshot 还吸收 `ContextUpdated` / `CompactionStarted/Finished/Failed` / `LongTaskStatus` / `ProposedPlan`，用于更新 context 用量、长任务进度和实施方案模块。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs::run_agent_chat_with_interleave` / `process_agent_chat` 以及 `handlers/im_gateway/event_loop.rs` 上的等价分支创建 progress session，并把 progress sender 注入 AgentSession。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` 维护 IM progress snapshot 和 Feishu streaming card session。
- `run_agent_chat_with_interleave` / `process_agent_chat` 创建 progress session，并把 progress sender 注入 AgentSession。
- `run_progress_event_coalescer` 对 status 类事件按 300ms 合并刷新，工具、计划、标题、过程文本、最终输出和结束事件立即刷新；Feishu session 继续按 section fingerprint 过滤未变化模块，避免 status-only 更新打出多次无效 CardKit API。
- `handle_busy_message` 在 guide / queue / remove queue 成功后通知 progress session 更新 snapshot。只有当前 progress session 仍处于 `Running` 时才执行 rollover：先在最新用户消息下方发送新卡片并把 registry handle 切到新 `card_id`，再 best-effort 把旧进度卡片更新为冻结快照并关闭 streaming；如果 progress session 已经 `Finished` / `Failed`，不能改写历史卡片，必须返回未更新并回退到普通确认消息或下一轮新卡片。
- `process_agent_chat` 在下一轮启动 progress session 时优先调用 `rollover_existing`：只有旧卡片仍处于 `Running` 时才把旧 snapshot 冻结到旧卡片、重置当前 snapshot、创建新的 CardKit card entity 并发送新 interactive 消息；如果旧卡片已终态，`rollover_existing` 返回 false，上层直接 `start_feishu` 新建下一张进度卡片。旧卡冻结失败只记录 warn，不阻断新卡片发送；新卡片发送失败则返回错误并保持旧 running card/handle。
- Feishu CardKit 整卡更新返回 `code=300305` 且错误信息包含 `element exceeds the limit` 时，progress session 复用 rollover 机制发送一张承载当前 snapshot 的新卡片，并把 registry handle 切到新 `card_id`；旧卡片冻结和关闭 streaming 仍是 best-effort，失败只记录 warn。完整卡片在正常渲染时主动控制 `agent_process_panel` 体积：执行过程只展示最近 30 次工具调用对应的 timeline 后缀，并在面板顶部明示前面已省略多少次工具调用，避免长任务上百轮调用提前触发 CardKit 元素大小限制。若新建完整 snapshot 卡片时 `create card entity` 仍返回 `code=300305`，说明当前 snapshot 自身已经超出 CardKit 限制，此时立即重试一张精简状态卡：只保留 status/footer、精简提示、截断后的最终输出或最新思考，不渲染 plan/process/tool 详情，并进入 compact card mode；后续 progress event / final close 继续更新这张精简卡，避免 Agent 仍在运行但飞书通道无任何状态。该恢复路径覆盖运行中 progress event、runner/status 刷新、restart 和 final flush；`finish` 完成后返回的新 `message_info` 必须指向恢复后的新卡片，便于 outbound log 和后续排查定位。
- Feishu IM send API 偶发返回 `code=230099` 且错误信息包含 `cardid is invalid` 时，说明刚创建出的 CardKit card entity 还不能被发送或已被 Feishu 判定失效。progress session 会重新创建一张 card entity 并重试发送一次；重试成功后 registry handle 指向新的 `card_id`，旧的无效 card entity 不再参与后续更新。外部 runner 的 `ProgressCard` delivery 在进度卡启动失败时不再发送“已开始处理 Runner 任务。”这类合成占位 IM，避免上一轮刚结束后下一轮退化时产生额外卡片；真正失败时仍保留最终回复 fallback。
- `run_agent_chat_with_interleave` 在 `process_agent_chat` 刚结束后、清理 guide/queue 前，会非阻塞 drain 当前 IM channel 中已经到达的事件，并复用 busy-message 处理逻辑把同 session 消息落入 guide/queue。这样模型正在输出最后回复或刚结束时到达的 IM 消息不会只被 ACK 而丢失。
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
- 过程文本使用普通 `agent_thinking_panel` markdown 元素，默认直接展示最后一次 `AssistantDelta` 的完整内容，不渲染折叠面板；同时 `agent_process_panel` 渲染包含 thinking / tool / status 条目的时间线，工具条目带 `ap_t` / `ap_td` / `ap_tg` 前缀的 element id，便于按 section fingerprint 增量更新。长任务的执行过程只保留最新 30 次工具调用对应的可见后缀，顶部摘要写明前面已省略的调用次数。
- guide/queue 更新会发送新 card entity，并把旧 card entity 更新为冻结快照、关闭 streaming，使消息流同时保留插话时刻的旧快照和最新进度卡片。
- queue 消息被消费为下一轮时，如果旧 progress session 仍处于 Running，progress registry 应创建新的 card entity / message，并 best-effort 冻结旧 card；冻结失败不得阻断新卡片发送。
- 已 Finished/Failed 的历史卡片收到新一轮消息时，progress registry 不得调用 freeze/rollover，不得改写旧 snapshot；上层应新建下一张 progress card。
- turn-end 窗口内已经到达 IM channel 的同 session 消息必须被 drain 到 guide/queue，并在当前轮完成后继续作为下一轮处理，不能只 ACK 后丢失。
- Feishu 撤回消息 API 仍保留独立方法并覆盖 tenant token 测试，但 progress card rollover 不调用 `DELETE /im/v1/messages/{message_id}`。
- CardKit 更新 uuid 长度保持在 Feishu 字段限制内；final flush 失败时仍尝试关闭 streaming。
- Feishu CardKit 整卡更新遇到 `code=300305 element exceeds the limit` 时应发送新 card entity，并把后续 progress event / final close 指向新 `card_id`；长任务执行过程应主动截断为最近 30 次工具调用并显示省略数量；如果新完整卡片创建也超限，应降级为 compact card mode，断言 compact payload 不包含超大 process/tool 详情但仍持续展示“Agent 仍在运行”和最新状态；旧卡片冻结失败不影响新卡片继续更新。
- Feishu send card entity 遇到 `code=230099 cardid is invalid` 时应重新创建 card entity 并重试一次；已结束历史卡片后的下一轮消息必须恢复为新的实时进度卡，不能发出“已开始处理 Runner 任务。”占位消息。

### E2E 测试

- 新增 IM Agent streaming card 回归，验证 progress renderer 输出 JSON 2.0 streaming card、固定组件 id、可选 plan/tool/thinking 模块、工具耗时和 guide/queue 状态区。
- 新增或复用 IM Gateway mock inbound E2E，验证 queue 消息进入下一轮时触发 `rollover_existing`，后续 progress event 指向新卡片；同时验证终态卡片不会被改写，turn-end 窗口消息会进入 guide/queue。
- 对真实 Agent loop 路径执行最小模型 mock，确认工具调用、计划更新、最终输出能进入 progress event。

### 真实场景测试

- 更新 `human_tests/im-gateway-agent.md`，新增流式进度卡片用例：
  - 正常 Agent loop：卡片持续更新，结束后关闭 streaming，标题跟随 `set_title`。
  - guide 消息：Running 旧卡片冻结并关闭 streaming，新卡片出现在最新用户消息之后，并展示待处理 guide。
  - queue 消息：Running 旧卡片冻结并关闭 streaming，新卡片出现在最新用户消息之后，并展示排队数量。
  - queue 消息成为下一轮：Running 旧卡片冻结，新卡片出现在最新用户消息下方，后续进展更新新卡片。
  - 已完成历史卡片：下一条独立新消息不撤回旧卡片，只发送新的进度卡片。
  - turn-end 窗口消息：模型最后输出/刚结束时发送的 IM 消息不丢失，会进入 guide/queue 并继续下一轮。
  - CardKit 卡片大小超限：运行中或 final flush 遇到 `code=300305 element exceeds the limit` 后，Bot 在下方发送新的进度卡片，后续进展和最终结论出现在新卡片，旧卡片只做 best-effort 冻结/关闭。
- 更新 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：确认四区卡片、Running 卡片 freeze-and-rollover、终态卡片保留、turn-end 消息不丢失和 provider-neutral 入口都已落地。
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
