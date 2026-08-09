# External Runner 子 Agent 可观测性

## 背景

Bifrost 会把 Codex、Trae X 与 Claude Code 的执行进度同时呈现在飞书 progress card 和 Web UI Agent Chat。现有归一化层只区分整体 run、thinking、plan 与普通 tool call。子 Agent 派发因此被压成普通工具事件：Codex / Trae X 的 `collabAgentToolCall` 在开始阶段读不到 `prompt`，Claude Code 的 `Task` / `Agent` 也只显示工具名。用户无法稳定看到派发任务、目标子 Agent、当前阶段、独立状态和耗时。

本文定义 provider-neutral 的子 Agent 生命周期、持久化和双端展示契约。

## 用户目标验证清单

### 必须实现

- Codex 与 Trae X 的 `collabAgentToolCall` / `subAgentActivity` 归一化为子 Agent 进度，保留任务 prompt、receiver thread、agent path、状态和错误信息。
- Claude Code 的 `Task` / `Agent` tool use/result 归一化到同一事件，保留 description、prompt、subagent type、agent id 与终态。
- 飞书卡片在执行过程区域提供独立的“子 Agent”条目类型，逐项展示任务、身份、当前阶段、状态和已运行/最终耗时。
- Web UI Agent Chat 提供独立子 Agent 行，不混入“已运行 N 条命令”；运行中耗时每秒更新，历史回放仍能恢复终态和耗时。
- 多个并发子 Agent 按稳定 ID 更新同一条记录，不能重复堆叠 started/updated/completed 快照。

### 必须不破坏

- 普通 command、MCP、file change、thinking、plan 与整体 run 终态继续使用原路径。
- 不要求所有 provider 同时提供同样完整的字段；缺失字段要以可读降级展示，不伪造 Agent ID 或阶段。
- 飞书卡片继续满足 CardKit 24 KiB / 180 component 标准预算和现有缩减策略。
- Web UI 使用 Ant Design token，亮色与暗色主题具有相同信息层级和可读性。
- 旧 history JSONL 没有 `subagent_updated` 时仍按现有 tool timeline 回放。

### 必须真实验证

- 用当前安装的 Codex / Trae X app-server schema 验证协议字段与状态枚举。
- 定向 Rust 单测覆盖 Codex / Trae X started、updated、completed、failed 与 Claude Code Task/Agent start/result。
- 定向 E2E 验证完整飞书 CardKit payload 和 Web timeline payload；本地不运行全量 E2E。
- Web UI 定向组件测试验证任务文本、状态、阶段、身份与运行中耗时，并检查亮/暗主题 token 使用。
- `human_tests/feishu-progress-card.md` 与新的 Web UI 子 Agent 场景逐条执行并记录实际结果。

### 必须交付

- 同步代码、单元测试、定向 E2E、human_tests 与索引。
- 完成至少两轮 Review/Fix/Test。
- 若修改生产 Rust，推送前通过 `make coverage-changed`；远端 CI 通过 `coverage-all.sh --json --gate` 90% 棘轮门禁。
- 提交、推送、创建或更新 PR，并看护远端 CI 到全绿或给出可核验证据。

## 协议证据

### Codex / Trae X

本机 `codex-cli 0.146.1` 与 `traecli 0.200.19` 生成的 app-server schema 均包含：

- `collabAgentToolCall`：`id`、`tool`、`status`、`prompt`、`senderThreadId`、`receiverThreadIds`、`agentsStates`、可选 model / reasoning effort。
- `CollabAgentTool`：`spawnAgent`、`sendInput`、`resumeAgent`、`wait`、`closeAgent`。
- `CollabAgentStatus`：`pendingInit`、`running`、`interrupted`、`completed`、`errored`、`shutdown`、`notFound`。
- `subAgentActivity`：`agentThreadId`、`agentPath`、`kind`，其中 kind 为 `started`、`interacted`、`interrupted`。

CLI JSONL 使用 snake_case item type，app-server 使用 camelCase item type。解析器必须同时兼容两者，且 app-server 需要消费 `item/started`、`item/updated` 与 `item/completed`。

### Claude Code

Claude Code stream-json 通过 assistant `tool_use` 发出 `Task` / `Agent`，输入常见字段为 `description`、`prompt`、`subagent_type`、`model`、`resume` 与 `run_in_background`；随后 user `tool_result` 返回结果，附加 `tool_use_result` 可能包含 agent id、耗时或 interrupted。归一化层只依赖已出现字段并保留未知字段作为 raw evidence。

## 统一状态模型

新增 provider-neutral `SubAgentProgress`：

- `id`：稳定更新键，优先使用 collab item/tool-use id。
- `agent_id`：receiver thread / Claude agent id，可缺省。
- `label`：agent path、subagent type 或 provider tool label。
- `task`：优先 prompt，其次 description；限制展示长度但持久化完整文本。
- `phase`：`dispatching`、`working`、`waiting`、`interacting`、`closing` 等可读阶段。
- `status`：`pending`、`running`、`completed`、`failed`、`interrupted`、`unknown`。
- `detail`：目标 Agent 的状态 message、错误或终态摘要。
- `started_at_ms` / `updated_at_ms` / `duration_ms`：用于独立耗时。缺少 provider 时间戳时在 Bifrost 首次观测时记时；终态冻结 duration。

状态映射优先使用目标 agent state，其次 collab tool call status，再使用事件 kind / tool 语义。等待父 Agent 的 `wait` 不把仍在工作的子 Agent 错标为完成。

## 数据流

1. external CLI / app-server parser 识别 provider 事件并产出 `ExternalCliProgressEventType::SubAgentUpdated`，raw 中保留统一字段。
2. `external_progress_to_agent_turn_event` 转成 `AgentTurnProgressEvent::SubAgentUpdated`，供飞书 progress registry 使用。
3. `ImAgentProgressSnapshot` 按稳定 ID upsert，记录首次开始时间并在终态冻结耗时。
4. 飞书卡片在 process panel 中渲染独立子 Agent 条目；预算收缩时优先移除最旧的终态条目，并至少保留最新 5 条终态与所有运行中条目。
5. Web NDJSON 直接携带统一字段；history recorder 写入 `subagent_updated`，Agent Chat 对 live 与 replay 使用同一 `ProcessStep` 结构。

## UI 约束

### 飞书卡片

- 条目首行使用“子 Agent”或 provider 提供的真实 label，并显示显式状态、阶段与耗时。
- 第二行显示“任务：…”；存在 detail 时提供短详情。
- 状态必须有文字，不能只依赖颜色或 emoji。

### Web UI

- 子 Agent 是独立 process item，不计入“命令”分组。
- 默认紧凑显示状态图标、label、阶段、耗时；任务文本保持一行可扫描，展开后显示完整任务、ID 与 detail。
- 运行中使用现有每秒 ticker 计算 elapsed；终态使用持久化 duration。
- 颜色与背景全部来自 Ant Design token，亮/暗主题不分叉。

## 测试方案

### 单元测试

- Codex / Trae X：snake_case CLI 与 camelCase app-server item 的 started、updated、completed、failed 状态和字段映射。
- Claude Code：Task / Agent tool use 与 success/error/interrupted tool result，验证 context 关联和耗时字段。
- progress snapshot：同 ID upsert、不重复、并发隔离、终态冻结 duration、缺字段降级。
- history：`subagent_updated` 记录与回放合并。
- Web helpers/components：子 Agent 不进入 command group，运行中耗时与终态状态正确。

### 定向 E2E

- 扩展单个 IM Gateway progress card renderer 场景，断言 CardKit payload 包含子 Agent 任务、ID、状态、阶段与耗时。
- 扩展 Agent Chat Playwright 单场景或 API fixture，断言 live/replay 展示一致。
- 按用户要求，本地不执行全量 E2E；远端 CI 负责完整套件。

### 真实场景测试

- 新增 `human_tests/external-runner-subagent-observability.md`，同时覆盖飞书卡片和 Agent Chat，包含 Codex、Trae X、Claude Code 与亮/暗主题。
- 文档写完后立即逐条执行并记录结果。

## Review/Fix/Test 闭环

### 第 1 轮

- 对照三种 provider 的真实 schema/fixture 复核字段兼容与状态优先级。
- 执行 `git status --short`、`git diff` 和必要的 `git diff --cached`。
- Review 并发 upsert、时间单位、终态冻结、CardKit 预算和 Web live/replay 一致性。
- 修复发现后运行定向 Rust、Web 与 E2E 用例。

### 第 2 轮

- 基于最新 diff 重新复核用户目标、第 1 轮修复和旧工具路径。
- 再次执行范围检查，核对 design / human_tests / 索引同步。
- 复跑受影响测试、`make coverage-changed` 与最终项目校验；如仍发现问题则追加轮次。

## 风险

- Provider 协议可能继续扩展字段或状态。解析器保留 raw 并使用别名读取，未知状态降级为 `unknown`，避免整条事件丢失。
- 飞书卡片不会因为本地时钟每秒自动刷新；它在 runner 新事件到来时更新运行中耗时，终态保证显示冻结的完整耗时。Web UI 可以每秒本地刷新。
- Claude Code 是否输出 agent id 取决于版本；缺失时稳定使用 tool-use id，不能显示虚构 ID。
