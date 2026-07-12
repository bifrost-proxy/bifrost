# ASR Daily Research Pipeline

## 目标

验证真实日报可以经过多 Agent 编排，形成结构化日报、研究种子、研究调度清单、每题独立研究会话、研究摘要和周度洞察；同时保证原始问题、GitHub 证据状态和完整 ChatGPT 链接不会在 Agent 之间丢失。

## 用例

### TC-ADRP-01：五段 Agent 链路与逐题独立会话

1. 创建 `daily_report -> research_seed -> research_dispatcher -> research_fanout -> research_digest` 五个 Agent。
2. Dispatcher 输出两个包含 `original_question` 的问题，其中一个指定 `ibkr-portfolio-dashboard`。
3. 运行同一天的完整 Daily Agent 链路。

预期：

- 两个问题生成不同的 conversation id 和 ChatGPT 链接。
- 每题目录包含 manifest、Markdown 报告和 JSON 元数据。
- fan-out 汇总及 digest 上游输入保留原始问题和两个完整链接。
- 一个问题失败不阻止另一个问题生成报告。

### TC-ADRP-02：Chat 模式和 Pro 模型硬校验

1. 将 ChatGPT Web Runner 配置为 `interfaceMode=chat`、`model=pro`。
2. 分别使用免费账号和 Pro 账号执行新研究会话。

预期：

- 免费账号缺少 Chat/Pro 控件时，Bifrost 在写入 Prompt 前失败，不发送问题。
- Pro 账号从 Work 切换为 Chat，选择并验证 Pro 后才写入和发送 Prompt。
- 失败产物明确记录是模式/模型校验失败。

### TC-ADRP-03：GitHub Connector 真实仓库核验

1. 在 Pro 账号授权并索引 `ibkr-portfolio-dashboard`。
2. 新开独立 ChatGPT 会话，要求解释成交记录来源和成本计算，并列出至少三个实际读取的文件。

预期：

- 报告包含 `GITHUB_CONNECTOR_STATUS: verified`。
- 文件路径确实存在于仓库，结论和源码一致。
- 如果仓库不可见，必须返回 `unavailable`；缺少状态标记时元数据为 `missing`，不能显示为已核验。

### TC-ADRP-04：GitHub 与真实交易数据边界

1. 研究仓库代码、表结构和查询口径。
2. 再研究用户真实成交、持仓、成本或现金记录。

预期：

- 仓库代码可以由 ChatGPT GitHub Connector 直接读取。
- 未提交到 GitHub 的 Supabase/IBKR 真实数据不会被声称为仓库内容。
- 需要真实运行时数据时，只能使用显式配置的本地 context profile；ChatGPT Web 不能作为本地 context Runner。

### TC-ADRP-05：配置页面

1. 打开某个 Daily Agent 的详情。
2. 开启 Independent Research。
3. 配置最大问题数、允许的研究 Runner 和可选 Runtime Data Fallback。

预期：

- 页面明确说明每题独立会话，以及 Chat + Pro 的发送前约束。
- 新增 fallback 必须先填写名称、Runner 和本地目录，未完整填写时不会保存无效配置。
- 每个 Agent 仍可独立编辑自己的 AGENTS.md Prompt。

### TC-ADRP-06：微信摘要与周度洞察

1. Daily report 使用摘要投递。
2. Research digest 投递每题原始问题、核心结论和完整 ChatGPT 链接。
3. 周度 Schedule 读取最近七天的日报、研究种子和研究结果。

预期：

- 微信消息不发送七篇完整流水账。
- 每题摘要均可点击进入对应独立 ChatGPT 会话。
- 周报只保留反复判断、未解决问题、形成中的方法论、产品机会、认知变化和下周优先项。

## 执行记录

| 日期 | 用例 | 结果 |
| --- | --- | --- |
| 2026-07-12 | TC-ADRP-01 | `bash e2e-tests/tests/test_asr_daily_agents_api.sh` 通过；五个 Agent 均产出 processed document，两个研究问题生成不同 conversation id 与链接。 |
| 2026-07-12 | TC-ADRP-02 免费账号负向用例 | 真实 run `1783850408000-b48c2cdf-711f-40fe-9171-0e481a2c4c90` 在发送前失败，错误为 `chat_mode_control_not_found`；诊断页面确认该独立 Chrome 登录的是免费账号，Prompt 未发送。 |
| 2026-07-12 | TC-ADRP-03 状态契约 | 单元测试覆盖 `verified / unavailable / missing`；Pro 账号真实 GitHub 读取等待用户在 Bifrost 独立 Chrome 完成账号切换。 |
| 2026-07-12 | TC-ADRP-04 | 检查 `ibkr-portfolio-dashboard`：GitHub 跟踪代码、Supabase schema 和读取逻辑；真实交易行不在 Git 跟踪文件中。context Runner 类型限制测试与编译通过。 |
| 2026-07-12 | TC-ADRP-05 | 前端 TypeScript 构建与 160 个单元测试通过；临时 18883 真实服务页面确认 fan-out 开关已开启、Max Questions=8、Runner=`chatgpt-web`、`ibkr_runtime` fallback 可见，页面明确提示每题独立会话及 Chat + Pro 约束。截图：`artifacts/asr-daily-research-ui.png`。 |
| 2026-07-12 | TC-ADRP-06 | 待 Pro 真实研究完成后执行微信与周度洞察人验。 |
