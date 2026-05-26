# Daily Agent 默认模板测试用例

## 功能模块说明

Daily Agent 默认模板会生成每个 ASR Directory Task 的 `daily/AGENTS.md`，指导 Agent 把每日转写整理成 `report/YYYY-MM-DD-report.md`。模板必须符合当前运行方式：输入是 AGENTS.md 与每日转写文件，输出是一份最终 report；长期知识沉淀应在 report 内分模块输出，不默认拆分到额外知识目录或文件。

## 前置条件

- 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
- 本用例只验证默认模板文本和生成路径约束，不需要启动 Bifrost 服务。
- 不修改用户真实 ASR 数据目录。

## 测试用例列表

### TC-DAT-01 默认模板不再引导生成 knowledge 目录

操作步骤：

1. 读取 `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md`。
2. 搜索模板中的知识沉淀相关段落。
3. 检查模板是否仍包含 `` `knowledge/`` 这类额外目录路径。

预期结果：

- 模板包含 `## 报告内知识沉淀模块`。
- 模板包含 `## 知识沉淀输出要求`。
- 模板明确要求所有知识沉淀内嵌在同一份 `{{report_dir}}YYYY-MM-DD-report.md`。
- 模板不包含 `` `knowledge/`` 目录路径。
- 模板没有要求或建议默认创建额外知识库文件。

### TC-DAT-02 报告结构按知识类型分模块输出

操作步骤：

1. 读取 `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md`。
2. 检查 `报告内知识沉淀模块` 下的分栏标题。
3. 检查这些分栏是否位于最终 report 结构代码块内，而不是独立文件规划段落。

预期结果：

- report 结构包含 `长期想法与效率方案`。
- report 结构包含 `方向决策与判断`。
- report 结构包含 `跨天待办追踪`。
- report 结构包含 `人物与协作背景`。
- report 结构包含 `学习与资料线索`。
- report 结构包含 `生活与状态线索`。
- report 结构包含 `术语与误识别词`。
- 模板表达的是同一份 report 内的模块化输出，不是多个新文档路径。

### TC-DAT-03 灵感爆发时刻包含资料搜索与可行性分析

操作步骤：

1. 读取 `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md`。
2. 检查 `灵感爆发时刻` 和 `灵感提取要求` 章节。
3. 确认模板要求每个灵感时刻都补充资料搜索、可行性分析、方案草案和下一步验证。
4. 确认模板要求无法联网搜索时必须显式标记，不能编造来源。

预期结果：

- 模板包含 `资料搜索结果`。
- 模板包含 `可行性分析`。
- 模板包含 `方案草案`。
- 模板包含 `未能联网搜索`。
- 模板要求输出关键发现和可参考来源，但不得编造来源。
- 每个灵感都能帮助用户快速判断方向是否值得继续投入。

### TC-DAT-04 默认模板头部提示是运行契约

操作步骤：

1. 读取 `crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_template.md`。
2. 检查模板开头是否说明该指南会写入 Daily Agent 工作目录的 `AGENTS.md`。
3. 检查模板是否明确这些提示是 Runner 实际读取的运行指令，而不是注释、示例或可忽略说明。
4. 检查模板是否明确核心模块不可省略，只能在无内容时写“无明确内容”。

预期结果：

- 模板包含 ``这份指南会作为当前 Daily Agent 工作目录中的 `AGENTS.md` 写入``。
- 模板包含 `下面所有规则都是运行指令，不是注释、示例或可忽略说明`。
- 模板包含 `不能省略会影响输出契约的核心模块`。
- 模板明确报告内知识沉淀分栏不是装饰性标题，而是承担日报、复盘、待办追踪和长期记忆索引的作用。

## 清理步骤

无需清理；本用例只读模板文件。

## 执行记录

| 日期 | 用例 | 命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-05-26 | TC-DAT-01 默认模板不再引导生成 knowledge 目录 | `source ~/.zshrc; bash e2e-tests/tests/test_asr_daily_agent_template.sh` | PASS：默认模板包含报告内知识沉淀模块和知识沉淀输出要求，明确同一份 `{{report_dir}}YYYY-MM-DD-report.md`，未发现 `` `knowledge/`` 目录路径 |
| 2026-05-26 | TC-DAT-02 报告结构按知识类型分模块输出 | `source ~/.zshrc; bash e2e-tests/tests/test_asr_daily_agent_template.sh` | PASS：所有模块标题均在默认模板 report 结构中存在，表达为同一份 report 内分模块输出 |
| 2026-05-26 | TC-DAT-03 灵感爆发时刻包含资料搜索与可行性分析 | `source ~/.zshrc; bash e2e-tests/tests/test_asr_daily_agent_template.sh` | PASS：模板要求每个灵感输出资料搜索结果、可行性分析、方案草案；无法联网搜索时必须显式说明，禁止编造来源 |
| 2026-05-26 | TC-DAT-04 默认模板头部提示是运行契约 | `source ~/.zshrc; bash e2e-tests/tests/test_asr_daily_agent_template.sh` | PASS：模板明确会写入 Daily Agent 工作目录的 `AGENTS.md`，所有规则是 Runner 实际读取的运行指令，核心模块不可省略，报告内知识沉淀分栏不是装饰性标题 |
