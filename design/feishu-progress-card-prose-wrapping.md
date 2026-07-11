# Feishu Progress Card Prose Wrapping

## 背景

飞书 CardKit 会尊重 markdown 内容中的硬换行。Codex app-server 对同一条 `agentMessage` 先发送多条 `item/agentMessage/delta` token 片段，再发送一条 `item/completed agentMessage` 完整正文。若两者都进入 progress timeline，卡片会把每个 token 渲染成独立一行，并再次展示完整正文。其它 reasoning、AssistantDelta、AssistantFinal 与工具事件仍是有效执行过程，不能整体过滤或合并。

## 目标

- 只忽略 Codex app-server 的 `item/agentMessage/delta` token 碎片，保留随后完整 `agentMessage` 作为一个 Progress Step。
- reasoning、其它 Runner 的 AssistantDelta、运行中的 AssistantFinal 和全部工具事件保持原有时间线与顺序。
- 展示层自然语言过程信息仍以正常段落展示，不保留单条完整事件内部的短行硬换行。
- 工具详情、命令输出、代码块、列表、表格等结构化 markdown 不能被合并。
- 修复点只作用于飞书 progress card 的过程/思考展示层，不改变 Agent timeline 持久化内容，也不影响 Web UI history。
- token usage 机器态事件不应作为执行过程状态行展示，但应作为卡片刷新信号更新尾部执行耗时。
- 卡片尾部执行耗时应使用秒/分钟/小时的可读格式，不展示毫秒级精度。

## 实现方案

- 在 `external_progress_to_agent_turn_event` 精确识别 raw method 为 `item/agentMessage/delta` 的事件并跳过映射；`item/completed agentMessage` 继续映射为 `AssistantFinal`。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs` 的 progress prose 归一化继续作为单条事件内部换行的展示层保护：
  - 对普通自然语言相邻非空行进行软合并。
  - 中文相邻片段直接拼接，ASCII 单词之间补一个空格。
  - 空行、代码围栏、列表、引用、标题、表格行保持原结构。
- `format_process_timeline_line` 的 Thinking/Status 以及 `format_thinking_markdown`、compact card 最新进展复用同一归一化函数。
- 工具输入/输出详情仍通过原有路径渲染，保留换行和 fenced code block。
- `ImAgentProgressSnapshot` 保留本轮首次 status 的 `started_at` 和最近 status 的 `updated_at`，footer 统一展示 `耗时：...`。
- 外部 Runner 的未知 usage progress event 会映射成一次 status refresh；progress card 会过滤 `token usage updated` 状态行，但 footer hash 因耗时变化而触发飞书卡片 patch。

## 验证计划

- 单元测试覆盖 Codex token delta 不映射、完整 agentMessage 仍映射为 Progress Step、普通 AssistantDelta 不受影响。
- 单元测试覆盖中文逐字换行合并、英文软换行补空格、列表与代码块保留、完整 Feishu card process content 不含异常硬换行。
- 单元测试覆盖执行耗时格式化、token usage status refresh 推进 footer 耗时且不进入过程状态行。
- E2E renderer 用例覆盖 progress card JSON 2.0 中的 process panel 内容。
- E2E renderer 用例覆盖 progress card JSON 2.0 footer 展示可读耗时。
- human_tests 用例从真实用户截图描述出发，验证同类短行思考内容在卡片 payload 中被合并为自然段。

## 风险

- raw method 精确匹配只覆盖 Codex app-server 协议；若协议改名，回归测试需同步更新。其它 Runner 和工具事件不受该过滤影响。
