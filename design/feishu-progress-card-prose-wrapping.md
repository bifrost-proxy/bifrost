# Feishu Progress Card Prose Wrapping

## 背景

飞书 CardKit 会尊重 markdown 内容中的硬换行。外部 Runner 的实时过程事件有时会把自然语言思考内容按 token 或短片段输出，例如每几个中文字符就带一个换行。Bifrost 之前在 `progress_card` 的执行过程面板中直接渲染这些换行，导致飞书移动端卡片里出现几字一行甚至近似竖排的 Progress Step/思考过程。

## 目标

- 飞书 progress card 的自然语言过程信息应以正常段落展示，不保留 token stream 带来的短行硬换行。
- 工具详情、命令输出、代码块、列表、表格等结构化 markdown 不能被合并。
- 修复点只作用于飞书 progress card 的过程/思考展示层，不改变 Agent timeline 持久化内容，也不影响 Web UI history。
- token usage 机器态事件不应作为执行过程状态行展示，但应作为卡片刷新信号更新尾部执行耗时。
- 卡片尾部执行耗时应使用秒/分钟/小时的可读格式，不展示毫秒级精度。

## 实现方案

- 在 `crates/bifrost-admin/src/im_gateway/progress_card.rs` 增加 progress prose 归一化：
  - 对普通自然语言相邻非空行进行软合并。
  - 中文相邻片段直接拼接，ASCII 单词之间补一个空格。
  - 空行、代码围栏、列表、引用、标题、表格行保持原结构。
- `format_process_timeline_line` 的 Thinking/Status 以及 `format_thinking_markdown`、compact card 最新进展复用同一归一化函数。
- 工具输入/输出详情仍通过原有路径渲染，保留换行和 fenced code block。
- `ImAgentProgressSnapshot` 保留本轮首次 status 的 `started_at` 和最近 status 的 `updated_at`，footer 统一展示 `耗时：...`。
- 外部 Runner 的未知 usage progress event 会映射成一次 status refresh；progress card 会过滤 `token usage updated` 状态行，但 footer hash 因耗时变化而触发飞书卡片 patch。

## 验证计划

- 单元测试覆盖中文逐字换行合并、英文软换行补空格、列表与代码块保留、完整 Feishu card process content 不含异常硬换行。
- 单元测试覆盖执行耗时格式化、token usage status refresh 推进 footer 耗时且不进入过程状态行。
- E2E renderer 用例覆盖 progress card JSON 2.0 中的 process panel 内容。
- E2E renderer 用例覆盖 progress card JSON 2.0 footer 展示可读耗时。
- human_tests 用例从真实用户截图描述出发，验证同类短行思考内容在卡片 payload 中被合并为自然段。

## 风险

- 如果 Runner 故意用普通行表达诗歌或特殊排版，该归一化会把它视为 prose 合并。当前作用范围仅限 progress thinking/status，不作用于最终回答和工具输出，风险可接受。
