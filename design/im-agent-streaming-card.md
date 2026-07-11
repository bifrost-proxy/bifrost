# IM Agent Streaming Progress Card 设计方案

## 背景

Bifrost IM Gateway 的 Agent 通道会在收到用户 IM 消息后触发 Agent loop：模型可能连续调用 shell / read / write 等工具、更新 plan、输出中间正文，最终给出回答。这一过程可能持续数十秒到数分钟。如果只在 loop 结束后一次性回复消息，用户在等待期间没有任何反馈，无法判断“Agent 是否还在工作、工作到哪一步、下一次输入会不会被吞掉”。

需要一张“进度卡”：在 loop 开始时立刻发出、随 Agent 事件更新模型思考/工具/plan/最终输出/状态、loop 结束后固化为终态卡片留在历史里。模型的流式中间正文需要归并为可读过程信号，并在 turn 收束时与最终输出去重。飞书 CardKit 支持 `streaming_mode=true` 的流式卡片，可以在同一 `card_id` 上局部更新组件；其它 IM 平台（微信、Webhook 等）能力较弱，本设计需要 provider-neutral 抽象让每个平台按能力落地。

## 用户目标验证清单

### 必须实现

- 每次 IM Agent loop 对应一张 CardKit 进度卡：收到 IM 消息立刻创建并发送，Agent loop 生命周期内更新固定组件，结束时写入终态并关闭 `streaming_mode`。
- 卡片区块包含：
  - 最终输出：执行中显示 `处理中...`，结束后显示最终回复，不额外渲染标题。
  - TodoList：仅当 Agent 调用 `update_plan` 后展示当前计划；折叠标题显示当前正在处理的任务。
  - 工具执行状态：仅当出现工具事件后渲染；默认折叠，折叠外显示最新工具名和状态。
  - 底部状态：默认折叠，折叠标题只展示 token 消耗；展开后展示 loop 状态、context 用量、压缩次数、排队消息、guide 状态、工作路径。
- 过程时间线（thinking / tool / status）在 `agent_process_panel` 中按序渲染，连续 assistant stream 归并为一条 thinking，超长任务只保留最近 30 次工具调用并显示已省略数量。
- 收到 guide/queue 消息时，Running 卡片被 best-effort 冻结（关闭 streaming、更新为快照），并在最新用户消息下方发送一张继承当前 snapshot 的新卡片，后续事件更新新卡片。
- loop 结束后进入 Finished/Failed 终态，历史卡片必须保留；后续独立新消息只能新建下一张卡片，不能改写已完成卡片。
- CardKit 整卡更新返回 `code=300305 element exceeds the limit` 时，自动新发一张承载当前 snapshot 的新卡片，把 registry handle 切到新 `card_id`；若新完整卡也超限则降级为 compact card 只保留 status/footer/截断输出。
- Feishu send card entity 返回 `code=230099 cardid is invalid` 时，重新创建 card entity 并重试一次；重试成功后 handle 指向新 `card_id`；重试仍失败时只保留最终 fallback 回复，不发合成的“已开始处理 Runner 任务”占位消息。
- 架构上不把 CardKit 逻辑写死为飞书私有代码：IM Gateway 提供 provider-neutral progress snapshot / renderer / capability 入口，Feishu 是第一版实现。

### 必须不破坏

- 已有的非流式 IM 回复路径（普通 `messages.send`、错误卡片、非 Agent 消息）仍能工作，不被强制走 progress card。
- Agent loop 内部事件模型（`AgentTurnProgressEvent`）保持向后兼容，新事件类型只是追加。
- Feishu tenant token / 消息 API / 撤回 API 的既有单元测试与调用路径不被 progress card 改动破坏。
- 已终态的历史 progress card 永远不被 rollover / freeze / delete 影响；不允许调用 `DELETE /im/v1/messages/{message_id}`。
- turn-end 窗口（模型正在输出最终回复到 progress task 收尾之间）到达的 IM 消息不能被 ACK 后丢失，必须 drain 到 guide/queue，作为下一轮消息继续处理。

### 必须真实验证

- 真实 Agent loop 触发 progress card 更新：肉眼观察卡片持续更新，工具面板出现、plan 变化、token 消耗、guide/queue 状态区提示、终态输出；中间 assistant 正文不重复出现。
- 真实 guide/queue 消息在 loop 运行中到达：旧卡片被冻结、新卡片出现在最新用户消息下方、后续进展更新新卡片。
- 长任务（>100 次工具调用）触发 `code=300305` 自动 rollover。
- 已完成历史卡片下 1 条新独立消息：不撤回旧卡片，只新发进度卡。
- turn-end 窗口消息不丢失：模型最后输出/刚结束时发送的 IM 消息进入 guide/queue 并继续下一轮。

## 产品语义

### 一次 Agent 可见执行 = 一张进度卡

“可见执行”指用户视角能感受到的一个 Agent 处理段。触发点：
- 收到普通 IM 消息 → Agent loop 启动 → 新卡片。
- guide 或 queue 消息进入 Running loop → 冻结旧卡片、发新卡片。
- queue 消息被消费为下一轮 → 冻结旧卡片（若仍 Running）、发新卡片。
- 独立新 IM 消息落到已完成卡片之后 → 直接新建下一张卡片，不动老卡片。

设计核心：消息流会同时保留“用户插话时刻的执行快照”，最新进度始终跟随最新用户消息。

### 四类可见信息 + 一类过程信息

- 最终输出（final）：始终位于卡片最后，是收束视觉。
- TodoList / 工具 / 底部状态：按存在与否可选渲染。
- 流式 assistant 正文归并后进入用户可见过程区；与 `TurnFinished` 最终正文等价的末尾 thinking 在收束时移除，避免重复。

产品原则：卡片默认信息密度低，用户扫一眼就知道“做到哪儿了、有没有问题、是否在等我下一步输入”。

### provider capability 描述能力，第一版 Feishu

后续 IM provider 接入时至少声明三类能力之一：

- `StreamingCard`：支持流式卡片创建、组件内容更新、关闭 streaming。Feishu CardKit 属于此类。
- `PatchMessage`：能原地更新发送后的消息，但没有真正 streaming。
- `SendOnly`：只能发新消息。仍可复用 snapshot，但无法同卡持续更新，需要以粗粒度快照消息呈现。

第一版只落地 `StreamingCard`（Feishu），其它平台按能力降级。

## 技术细节

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

IM Gateway 把这些事件归并为 `ImAgentProgressSnapshot`：

```rust
pub struct ImAgentProgressSnapshot {
    pub session_key: String,
    pub title: Option<String>,
    pub output: Option<String>,
    pub last_thought: Option<String>,
    pub runner: RunnerSummary,          // adapter / model / token usage / work_dir
    pub plan_steps: Vec<PlanStep>,
    pub proposed_plan: Option<ProposedPlan>,
    pub tool_calls: Vec<ToolCallSummary>,
    pub latest_tool: Option<ToolCallSummary>,
    pub timeline: Vec<TimelineItem>,    // thinking / tool / status
    pub status: ProgressStatus,
    pub context: ContextUsage,          // 用量 + 压缩计数
    pub queue_items: Vec<QueueItem>,
    pub guide_pending: Option<GuideStatus>,
    pub activity_notice: Option<String>,
    pub phase: ProgressPhase,           // Running / Finished / Failed
}
```

snapshot 是所有 renderer 的共同输入。

### Feishu CardKit 生命周期

```text
IM message received
  -> create CardKit card entity with streaming_mode=true
  -> send interactive message with card_id
  -> Agent loop emits progress events
  -> coalesce progress events (status 300ms 合并；工具/plan/标题/过程/最终立即刷新)
  -> update element content:
       output / optional plan / optional tool panel
       / optional process timeline (agent_process_panel)
       / folded status / visible thought
  -> guide 或 queue 消息:
       -> create and send a new CardKit card entity below latest user message
       -> best-effort update previous card entity as frozen Finished snapshot
       -> best-effort close previous card streaming_mode=false
  -> CardKit update returns code=300305 element exceeds the limit:
       -> create and send a new CardKit card entity with latest snapshot
       -> if the new full snapshot card also returns code=300305, retry once with compact status card
       -> subsequent updates target the new card_id
       -> best-effort freeze/close the oversized previous card
  -> loop finished:
       -> flush final output
       -> close streaming_mode=false
  -> queued message becomes next turn:
       -> if previous card is still Running, create new card and freeze/close previous
       -> if previous card is Finished/Failed, keep it in history and create new card directly
       -> subsequent updates target the new card_id
  -> send card entity returns code=230099 cardid is invalid:
       -> treat just-created CardKit entity as unusable
       -> create fresh CardKit card entity and retry sending once
       -> if retry succeeds, updates target fresh card_id
       -> if retry still fails, do not emit synthetic "started runner task" IM message;
          only send final fallback reply when the run finishes
```

### 组件 element_id 约定

- `agent_output_panel` — 最终输出/`处理中...`。
- `agent_plan_panel` — TodoList（component-level update）。
- `agent_tool_panel` — 工具执行状态（default folded）。
- `agent_process_panel` — 过程时间线（thinking / tool / status），条目 element id 前缀 `ap_t` / `ap_td` / `ap_tg`，便于按 section fingerprint 增量更新。
- `agent_status_panel` — 底部状态区（default folded，标题只展示 token）。

Section fingerprint 用于过滤未变化模块，避免 status-only 事件反复打无效 CardKit API。

## 实现逻辑

- `crates/agent`：新增 progress event 通道，挂在 `AgentSession` 上。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`：维护 IM progress snapshot 和 Feishu streaming card session；除基础 status/plan/tool/final 外，snapshot 还吸收 `ContextUpdated` / `CompactionStarted/Finished/Failed` / `LongTaskStatus` / `ProposedPlan`，用于更新 context 用量、长任务进度和实施方案模块。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs::run_agent_chat_with_interleave` / `process_agent_chat` 以及 `handlers/im_gateway/event_loop.rs` 上的等价分支：创建 progress session，并把 progress sender 注入 `AgentSession`。
- `run_progress_event_coalescer`：对 status 类事件按 300ms 合并刷新；工具、计划、标题、assistant 正文、最终输出和结束事件立即刷新；assistant 正文增量在 progress snapshot 边界归并；Feishu session 继续按 section fingerprint 过滤未变化模块。
- `handle_busy_message`：在 guide / queue / remove queue 成功后通知 progress session 更新 snapshot。只有当前 progress session 处于 `Running` 时才执行 rollover：先在最新用户消息下方发送新卡片、把 registry handle 切到新 `card_id`，再 best-effort 把旧卡片更新为冻结快照并关闭 streaming；若 progress session 已 `Finished` / `Failed`，不改写历史卡片，返回未更新并回退到普通确认消息或下一轮新卡片。
- `process_agent_chat` 在下一轮启动 progress session 时优先调用 `rollover_existing`：只有旧卡片仍 `Running` 时才把旧 snapshot 冻结到旧卡片、重置当前 snapshot、创建新的 CardKit card entity 并发送新 interactive 消息；若旧卡片已终态，`rollover_existing` 返回 false，上层直接 `start_feishu` 新建下一张进度卡片。旧卡冻结失败只 warn，不阻断新卡发送；新卡发送失败则返回错误并保持旧 running card/handle。
- CardKit 整卡更新 `code=300305 element exceeds the limit` 且错误信息含 `element exceeds the limit` 时，progress session 复用 rollover 机制发送一张承载当前 snapshot 的新卡片，把 handle 切到新 `card_id`；旧卡冻结与关闭 streaming 仍 best-effort。完整卡片主动控制 `agent_process_panel` 体积：执行过程只展示最近 30 次工具调用对应的 timeline 后缀，并在面板顶部明示前面已省略多少次工具调用。若新完整 snapshot 卡在 `create card entity` 时又返回 `code=300305`，立即重试一张精简状态卡：只保留 status/footer、精简提示和截断后的最终输出，不渲染 plan/process/tool 详情，并进入 compact card mode；后续 progress event / final close 继续更新这张精简卡。恢复路径覆盖运行中 progress event、runner/status 刷新、restart 和 final flush；`finish` 返回的新 `message_info` 必须指向恢复后的新卡片。
- Feishu IM send API 偶发返回 `code=230099` 且错误信息含 `cardid is invalid` 时，重新创建 card entity 并重试一次；重试成功后 handle 指向新 `card_id`，旧无效 card entity 不再参与后续更新。外部 runner 的 `ProgressCard` delivery 在进度卡启动失败时不再发送“已开始处理 Runner 任务。”占位 IM。
- `run_agent_chat_with_interleave` 在 `process_agent_chat` 刚结束后、清理 guide/queue 前，非阻塞 drain 当前 IM channel 中已到达的事件，并复用 busy-message 处理逻辑把同 session 消息落入 guide/queue。模型正在输出最后回复或刚结束时到达的 IM 消息不会被 ACK 丢失。
- `set_title` 工具刷新标题时，通过 CardKit 整卡更新刷新 header；没有工具标题时初始标题使用用户消息。
- CardKit 更新 uuid 使用短随机值，不拼接 `card_id`；loop 结束时即使最终内容 flush 失败，也 best-effort 关闭 `streaming_mode=false`。
- progress outbound message log 记录真实 Feishu `message_id`，并把 CardKit `card_id` 写入 target 线索。

## CLI + Web + Admin API

本设计对外无独立 CLI / Web / Admin API 入口：

- 无 `bifrost im card` 子命令。
- Web 不新增页面；进度卡由 Agent loop 自动创建，只能在飞书客户端观察。
- Admin API 不新增路径。
- Remote Invoke：progress card 不出现在 `remote_im_gateway` 的独立 grant 里，它是 send message 的实现细节。

Outbound message log 中会包含真实 Feishu `message_id` 与 CardKit `card_id`，便于运维在 History 页面追踪。

## Sync 边界

- Progress snapshot 只在内存中维护，不持久化到 sync。
- Card ID / message ID 只作为本地 outbound message log，Sync 中不再镜像。
- Remote Invoke 不能读取 raw CardKit payload；`bifrost remote im ...` 只能读 outbound message log 的 safe summary。

## 实现切分

### Phase 1：progress snapshot 与 event 通道

- `crates/agent` 新增 progress event 通道并挂到 `AgentSession`。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` 建立 `ImAgentProgressSnapshot` 与状态机。
- `run_progress_event_coalescer` 合并 status 类事件。
- 单元测试：snapshot 从 status / plan / tool / final 事件归并。

### Phase 2：Feishu CardKit renderer

- 实现 CardKit card entity 创建、发送、组件更新、关闭 streaming。
- 组件 element_id 固化：output / plan / tool / process / status。
- Section fingerprint 过滤未变化模块。

### Phase 3：guide/queue rollover

- `handle_busy_message` 通知 progress session。
- Running 卡片冻结 + 新卡发送 + registry handle 切换。
- 已终态卡片跳过 rollover，直接新建下一张。
- `process_agent_chat` 下一轮调用 `rollover_existing`。

### Phase 4：failure 降级 + turn-end drain

- `code=300305` rollover + compact card fallback。
- `code=230099` 重建 card entity 重试一次。
- `run_agent_chat_with_interleave` drain turn-end 窗口消息到 guide/queue。
- 外部 runner ProgressCard delivery 失败时不再发合成占位消息。

### Phase 5：文档与真实场景测试

- 更新 `human_tests/im-gateway-agent.md` 新增流式进度卡片用例。
- 更新 `human_tests/readme.md` 索引。

## 测试方案

### 单元测试

- Progress snapshot 从 status / plan / tool / final 事件归并出稳定卡片内容。
- guide / queue 进入 snapshot 后，footer 显示排队数量和 guide pending 状态。
- Feishu streaming card JSON 包含 JSON 2.0、`streaming_mode=true`、固定 element_id。
- 无计划、无工具、无思考或可读状态时不渲染对应过程模块。
- 工具执行状态使用默认折叠的 `agent_tool_panel`，折叠标题展示最新工具名、成功/失败、耗时和累计次数。
- 状态区使用默认折叠的 `agent_status_panel`，折叠标题只展示 token 消耗。
- guide/queue 注入后，状态区标题追加“已收到引导 / 已加入排队 / 已删除排队”的轻量提示。
- 计划面板使用 `agent_plan_panel` 组件级更新标题和内容，标题优先展示 in-progress step。
- `AssistantDelta` 与运行中的 `AssistantFinal` 进入 progress snapshot 并按连续 stream 归并；`agent_process_panel` 展示思考、工具与可读状态，条目带 `ap_t` / `ap_td` / `ap_tg` element id。`TurnFinished` 只移除与最终正文等价的末尾 thinking。长任务只保留最近 30 次工具调用的可见后缀，顶部摘要写明前面已省略的调用次数。
- guide/queue 更新发送新 card entity，并把旧 card entity 更新为冻结快照、关闭 streaming。
- queue 消息被消费为下一轮时，若旧 progress session 仍 Running，registry 创建新 card entity / message，并 best-effort 冻结旧 card；冻结失败不阻断新卡发送。
- 已 Finished/Failed 的历史卡片收到新一轮消息时，registry 不调用 freeze/rollover，不改写旧 snapshot；上层新建下一张 progress card。
- turn-end 窗口内已到达 IM channel 的同 session 消息必须 drain 到 guide/queue，作为下一轮处理。
- 撤回消息 API 保留独立方法并覆盖 tenant token 测试，但 progress card rollover 不调用 `DELETE /im/v1/messages/{message_id}`。
- CardKit 更新 uuid 长度保持在飞书字段限制内；final flush 失败时仍尝试关闭 streaming。
- `code=300305 element exceeds the limit` 应发送新 card entity，后续事件指向新 `card_id`；长任务执行过程应主动截断为最近 30 次工具调用并显示省略数量；若新完整卡片创建也超限，应降级为 compact card mode，断言 compact payload 不含超大 process/tool 详情但仍持续展示“Agent 仍在运行”和最新状态；旧卡片冻结失败不影响新卡片继续更新。
- Feishu send card entity 遇到 `code=230099 cardid is invalid` 时应重新创建 card entity 并重试一次；已结束历史卡片后的下一轮消息必须恢复为新的实时进度卡，不发合成占位消息。

### E2E 测试

- 新增 IM Agent streaming card 回归：验证 progress renderer 输出 JSON 2.0 streaming card、固定组件 id、可选 plan/tool/thinking 模块、assistant stream 归并、最终正文只出现一次、工具耗时和 guide/queue 状态区。
- 新增或复用 IM Gateway mock inbound E2E：验证 queue 消息进入下一轮时触发 `rollover_existing`，后续 progress event 指向新卡片；同时验证终态卡片不会被改写，turn-end 窗口消息会进入 guide/queue。
- 对真实 Agent loop 路径执行最小模型 mock，确认工具调用、计划更新、最终输出能进入 progress event。

### 真实场景测试

更新 `human_tests/im-gateway-agent.md`，新增流式进度卡片用例：

- TC-ASC-01：正常 Agent loop 卡片持续更新，结束后关闭 streaming，标题跟随 `set_title`。
- TC-ASC-02：guide 消息 → Running 旧卡片冻结并关闭 streaming，新卡片出现在最新用户消息之后，并展示待处理 guide。
- TC-ASC-03：queue 消息 → Running 旧卡片冻结并关闭 streaming，新卡片出现在最新用户消息之后，并展示排队数量。
- TC-ASC-04：queue 消息成为下一轮 → Running 旧卡片冻结，新卡片出现在最新用户消息下方，后续进展更新新卡片。
- TC-ASC-05：已完成历史卡片 → 下一条独立新消息不撤回旧卡片，只发新的进度卡片。
- TC-ASC-06：turn-end 窗口消息 → 模型最后输出/刚结束时发送的 IM 消息不丢失，会进入 guide/queue 并继续下一轮。
- TC-ASC-07：CardKit 卡片大小超限 → 运行中或 final flush 遇到 `code=300305` 后 Bot 在下方发送新的进度卡片，后续进展和最终结论出现在新卡片，旧卡片只 best-effort 冻结/关闭。

更新 `human_tests/readme.md` 索引。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 受影响 crate 的单元测试。
- 新增/更新 E2E。
- `cargo test --workspace --all-features`
- 按修改范围评估 `scripts/ci/local-ci.sh`。

本机 no-local-coverage 约定下不跑 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：确认四区卡片、Running 卡片 freeze-and-rollover、终态卡片保留、turn-end 消息不丢失、provider-neutral 入口全部落地。
- 代码 review：重点检查 Feishu API request 序列、进度 task 生命周期、并发锁、失败降级、旧普通卡片路径兼容。
- 测试：运行 progress/feishu/card/queue 相关单元测试与新增 E2E。

### 第 2 轮

- 再次复核 diff、docs、`human_tests` 索引和真实执行记录。
- 复跑第 1 轮失败或修复过的路径。
- 若发现 card lifecycle、queue/guide 或 status 同步缺口，继续追加轮次。

## 风险与决策点

- **不支持卡片内交互按钮**：Feishu streaming mode 处理卡片回调需要先关闭 streaming，会扩大状态机复杂度。第一版留待后续。
- **不承诺模型 token delta 真流式**：若模型客户端未暴露 delta，最终输出在 loop 结束时一次性写入；计划、工具、状态仍持续更新。
- **不为所有 IM provider 实现原地更新**：provider 能力由 capability 描述，后续接入者按能力降级。
- **Provider identity 不复用 Remote Invoke caller**：progress card 只是本地 IM Gateway 的实现细节，不进入 Remote grant 语义。
- **卡片大小限制**：`code=300305` 通过 rollover + compact fallback 逃生；如飞书调整上限，需重新校准 30-tool 截断窗口。
- **多实例 provider 冲突**：飞书长连接同应用最多 50 连接，集群模式随机投递。第一版不共享 provider secret；若检测到同 provider 多实例在线，只在 UI 提示不做协调。
