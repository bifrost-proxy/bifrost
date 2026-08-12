# Feishu Progress Card Assistant Stream Coalescing

## 背景

外部 Runner 会持续产生 `AssistantDelta` / 运行中的 `AssistantFinal`，随后 `TurnFinished` 又携带完整最终回复。过程正文必须实时出现在飞书卡片中，否则用户只能看到最终结果，无法了解模型在做什么；但若把每个 token/字符都追加成独立 timeline item，同一段思考会近似竖排，最终正文还会在过程区和卡片底部重复。

PR #370 通过丢弃全部 `AssistantDelta` 和 Running 阶段的 `AssistantFinal` 消除了重复，同时也误删了用户需要的执行过程。本设计改为在 progress snapshot 边界归并同一条 assistant stream，并只在 turn 收束时移除与最终答案等价的末尾过程项。

## 目标

- `AssistantDelta` 与运行阶段的 `AssistantFinal` 继续进入飞书 progress snapshot，并实时展示模型思考/阶段说明。
- 连续 token/字符增量、累计快照和随后到达的完整 `AssistantFinal` 只归并成一条 thinking timeline item。
- `TurnFinished` 的完整正文只在卡片底部最终输出区域展示一次。
- 工具调用、计划、可读状态、长任务状态和失败信息继续进入“执行过程”，且 assistant 去重不能影响这些事件。
- 修复只作用于 IM progress snapshot，不改变 Runner 事件解析、内部统计、最终响应选择或 Web UI timeline。
- token usage 机器态事件不应作为执行过程状态行展示，但应作为卡片刷新信号更新尾部执行耗时。
- 卡片尾部执行耗时应使用秒/分钟/小时的可读格式，不展示毫秒级精度。

## 实现方案

- `AssistantDelta` 更新 timeline 末尾连续的 thinking item：累计快照取较完整版本，独立 token/字符片段拼接到同一 item，而不是逐片段新增条目。
- Running 阶段的 `AssistantFinal` 若与末尾 thinking 互为完整/前缀快照，则用 final 收敛该 item；若内容不同则作为新的阶段性过程项保留。
- Codex App Server 会先把公开 reasoning summary 和 commentary delta 归并到同一 thinking item，再用 `AssistantFinal` 发送 commentary 完整快照。若该 final 已是 thinking item 的规范化后缀，说明正文已流式写入：保留前面的 reasoning summary 和现有正文，不再追加或用 final 覆盖整条 item。
- `TurnFinished` 负责设置 `snapshot.output`，并仅移除与最终正文忽略空白后等价的末尾 thinking item；前面的思考与工具记录保留。
- 工具输入/输出、可读状态和错误仍通过原有 timeline 路径渲染。
- 原有 prose 归一化继续服务可读状态文本，不再承担 assistant 正文去重职责。
- `ImAgentProgressSnapshot` 保留本轮首次 status 的 `started_at` 和最近 status 的 `updated_at`，footer 统一展示 `耗时：...`。
- 运行态卡片维护独立于模型事件的本地展示时钟。任意可见进度刷新后若连续 10 秒没有新事件，由每个 session 唯一的保活循环刷新稳定的 `agent_output` 与 `agent_status_panel` 元素；不追加 timeline、不重建整卡，进入 Finished/Failed、session 被替换或句柄失效后停止。
- 运行态底部固定展示 `处理中... · 耗时：... · 最后更新：YYYY-MM-DD HH:mm:ss`，时间使用 Bifrost 所在设备的本地时区；每次模型事件刷新或保活刷新都会推进。耗时采用秒、分秒、时分秒、天时分秒的自适应格式，省略值为 0 的前导高位单位，但一旦出现高位单位就保留两位低位字段，避免计时文本宽度反复跳变。
- 外部 Runner 的未知 usage progress event 会映射成一次 status refresh；progress card 会过滤 `token usage updated` 状态行，但 footer hash 因耗时变化而触发飞书卡片 patch。

## 验证计划

- 单元测试覆盖逐字符 `AssistantDelta`、累计快照和运行中 `AssistantFinal` 归并成单条可读 thinking item。
- 单元测试覆盖中间思考与工具事件仍可见，且 `TurnFinished` 只移除末尾重复正文，最终正文在 card body 中只出现一次。
- 单元测试与隔离 Service + mock Runner + mock Feishu OpenAPI 黑盒 E2E 覆盖 `reasoning summary → commentary delta → 相同 commentary final`，断言每轮 reasoning 和 commentary 在执行过程各出现一次；工具边界后的下一轮同样成立。
- 单元测试覆盖执行耗时格式化、token usage status refresh 推进 footer 耗时且不进入过程状态行。
- 单元测试覆盖 10 秒静默阈值、设备本地时间格式、保活只更新既有状态/底部元素，以及终态不再保活；隔离 Service + mock Runner + mock Feishu OpenAPI 黑盒 E2E 覆盖 Runner 静默超过 10 秒时自动产生保活更新。
- E2E renderer 用例覆盖 progress card JSON 2.0 中包含归并后的 assistant 过程、工具 process panel 与单次 final。
- E2E renderer 用例覆盖 progress card JSON 2.0 footer 展示可读耗时。
- human_tests 用例验证同类短行思考内容显示为一个自然段，模型阶段说明、工具和计划仍可见，最终结果不重复。

## 风险

- 不同 Runner 对 delta 的语义可能是增量片段或累计快照；归并逻辑同时处理前缀快照和独立片段。若两个本应分开的自然语言事件之间没有任何边界事件，它们可能被合并到同一 thinking item，但正文不会丢失，且工具/计划事件会形成稳定边界。
- 后缀判断只用于 Running 阶段的终态快照确认，不用于普通 `AssistantDelta`，因此 `"哈" + "哈"` 等合法重复 token 仍会保留，不进行基于内容长度的全局武断去重。
