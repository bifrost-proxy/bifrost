# Feishu Progress Card Assistant Stream Suppression

## 背景

外部 Runner 会持续产生 `AssistantDelta` / 运行中的 `AssistantFinal`，随后 `TurnFinished` 又携带完整最终回复。把前两类正文写入飞书卡片“执行过程”，会同时造成两个用户问题：token/字符片段在 CardKit 中近似竖排；同一回答在“执行过程”和卡片底部重复出现。

此前仅在展示层合并短行，只处理了竖排外观，没有消除重复信息。产品语义应以事件分层为准，而不是继续修补流式正文排版。

## 目标

- `AssistantDelta` 与运行阶段的 `AssistantFinal` 不进入飞书 progress snapshot，也不出现在“执行过程”。
- `TurnFinished` 的完整正文只在卡片底部最终输出区域展示一次。
- 工具调用、可读状态、长任务状态和失败信息继续进入“执行过程”。
- 修复只作用于 IM progress snapshot，不改变 Runner 事件解析、内部统计、最终响应选择或 Web UI timeline。
- token usage 机器态事件不应作为执行过程状态行展示，但应作为卡片刷新信号更新尾部执行耗时。
- 卡片尾部执行耗时应使用秒/分钟/小时的可读格式，不展示毫秒级精度。

## 实现方案

- `ImAgentProgressSnapshot::apply_event` 丢弃 `AssistantDelta`，并在 Running 阶段丢弃 `AssistantFinal`。
- `TurnFinished` 仍负责设置 `snapshot.output`；非 Running 阶段的 `AssistantFinal` 保留兼容兜底。
- 工具输入/输出、可读状态和错误仍通过原有 timeline 路径渲染。
- 原有 prose 归一化继续服务可读状态文本，不再承担 assistant 正文去重职责。
- `ImAgentProgressSnapshot` 保留本轮首次 status 的 `started_at` 和最近 status 的 `updated_at`，footer 统一展示 `耗时：...`。
- 外部 Runner 的未知 usage progress event 会映射成一次 status refresh；progress card 会过滤 `token usage updated` 状态行，但 footer hash 因耗时变化而触发飞书卡片 patch。

## 验证计划

- 单元测试覆盖逐字符 `AssistantDelta` 和运行中 `AssistantFinal` 不进入 timeline/process/output。
- 单元测试覆盖工具事件仍可见，且 `TurnFinished` 最终正文在 card body 中只出现一次。
- 单元测试覆盖执行耗时格式化、token usage status refresh 推进 footer 耗时且不进入过程状态行。
- E2E renderer 用例覆盖 progress card JSON 2.0 中不含 assistant stream，仍含工具 process panel 与单次 final。
- E2E renderer 用例覆盖 progress card JSON 2.0 footer 展示可读耗时。
- human_tests 用例从真实用户截图描述出发，验证同类短行思考内容完全不进入执行过程。

## 风险

- 卡片不再实时展示模型自然语言 commentary，等待期间主要依靠计划、工具和状态反馈。该取舍消除重复输出，并保留真正可操作的过程信号。
