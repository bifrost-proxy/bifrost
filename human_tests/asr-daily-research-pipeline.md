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
3. 在 Pro 账号页面存在侧边栏、Project 操作等多个可见 `aria-haspopup="menu"` 按钮时，保持模型按钮显示 `Pro`，再从真实 Project 发起一次独立研究。
4. Project 中预先保留其他历史研究会话；新研究 handoff 返回 conversation id 后，核对最终报告正文和链接均属于该 id，而不是历史会话内容。
5. 让首条回复只返回“我会先检索……”一类规划句，再观察系统是否在同一 conversation id 内要求完整最终报告。

预期：

- 免费账号缺少 Chat/Pro 控件时，Bifrost 在写入 Prompt 前失败，不发送问题。
- Pro 账号从 Work 切换为 Chat，选择并验证 Pro 后才写入和发送 Prompt。
- 页面存在多个无关菜单按钮时，系统从全部候选中识别唯一 `Pro` 模型按钮并继续发送；如果出现多个 Pro 或多个模型候选，则保持失败关闭，不猜测点击。
- 创建下一题的新会话时，不关闭前一题的 Project conversation tab；前一题即使仍在后台深度研究，也能由后续 `wait` 重新 attach 并取回最终稿。
- DOM fallback 只从本次 handoff 对应的普通或 Project conversation URL 取结果；页面仍处在其他会话或 Project 入口时继续等待，不返回历史会话的短计划前缀。
- 以“我会 / 我将 / I'll / I will / Let me”等开头的短规划句进入 15 秒观察窗；深度研究 busy 控件在间隙后恢复时继续等待同一轮最终正文，不提前结束 adapter run。
- `Succeeded` 但长度不足、缺少原始问题或缺少五个规定章节的回复不计为研究成功；系统先在同一会话执行纯等待并重新提取，仍不完整时才自动补发一次最终输出要求，重试仍不完整则 child 失败，不能进入 digest 或微信。
- 隔离 E2E 的 ChatGPT Web mock 首轮固定返回短规划句；run artifacts 至少出现每题一个空 `prompt.md` 的 wait 子 run和一个最终输出 retry prompt，最终 child 报告按顺序包含五个独立章节及各节正文，不接受提示词回显。
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
4. 在 Project URL、已有 fallback 目录和说明中连续输入文本，观察保存时机。

预期：

- 页面明确说明每题独立会话，以及 Chat + Pro 的发送前约束。
- 新增 fallback 必须先填写名称、Runner 和本地目录，未完整填写时不会保存无效配置。
- 长文本字段只在失焦或按回车时保存一次，不会逐字符发请求或被乱序响应覆盖。
- 每个 Agent 仍可独立编辑自己的 AGENTS.md Prompt。

### TC-ADRP-06：微信摘要与周度洞察

1. Daily report 使用摘要投递。
2. Research digest 投递每题原始问题、核心结论和完整 ChatGPT 链接。
3. 周度 Schedule 读取最近七天的日报、研究种子和研究结果。

预期：

- 微信消息不发送七篇完整流水账。
- 每题摘要均可点击进入对应独立 ChatGPT 会话。
- 周报只保留反复判断、未解决问题、形成中的方法论、产品机会、认知变化和下周优先项。

### TC-ADRP-07：“帮我记录一下”研究意图精确分流

1. 输入用户确认的真实研究正样本：华为“韬”定律、SpaceX、Claude Managed Agents、Codex Sub Agent、播客声纹翻译、微软“企业数字基础设施”、段永平微软持仓、金建成修正系数、信贷与储蓄/长短周期、阿里研究。
2. 同时输入线上超时、报警屏蔽、Proxy 拦截失效、Session 队列、TCC/PPE 差异、消息重复、修 Bug、查日志等记录事项。
3. 使用默认 `research_seed` Prompt 生成分类结果，再由 `research_dispatcher` 生成 manifest。

预期：

- 正样本进入 `research_questions`，并保留原始问题与上下文。
- 运行故障和执行事项进入 `action_item` 或 `internal_investigation`，不会进入 manifest。
- “帮我记录一下”本身不触发研究；没有真正研究问题时允许输出空数组。
- 不输出研究优先级或分数。

### TC-ADRP-08：ChatGPT Project 归档与独立新会话

1. 在 Independent Research 中填写一个真实 ChatGPT Project URL。
2. 使用带 `bifrost_new_chat` nonce 的 Project URL 打开新研究入口。
3. 确认页面仍是指定 Project，且 Chat 模式、Pro 模型和 Project 专用 composer 同时可见。
4. 发送一个只返回固定标记的冒烟问题，并把至少一个已有研究会话通过“移至项目”归档。
5. 使用同一个 Project 连续创建多个研究问题。

预期：

- 配置保存后保持规范的 `https://chatgpt.com/g/g-p-<project-id>/project`；错误 host、普通 conversation URL 和非 Project 路径被拒绝。
- ChatGPT 自动增加可读 slug 时仍按稳定 project id 识别为同一 Project。
- 冒烟问题生成 `/g/g-p-.../c/<conversation-id>` 项目内链接，且 Pro 回复固定标记。
- 历史研究和新研究都出现在 Project 的“聊天”列表；不同问题仍是不同 conversation，不共享一条聊天。
- 页面若跳回普通首页或其他 Project，Bifrost 在写入 Prompt 前失败，不把问题发送到项目外。

### TC-ADRP-09：空研究清单正常收敛

1. 使用只包含内部故障或执行事项、没有外部研究问题的日报运行五段 Agent 链路。
2. 确认 `research_dispatcher` 输出 `{"questions":[]}`。
3. 检查 fan-out 产物和下游 `research_digest`。

预期：

- fan-out 成功完成，保存空的 `manifest.json`，且不创建任何 child run。
- 日期级 fan-out report 明确写明当日没有需要外部研究的问题。
- `research_digest` 继续执行并生成可投递摘要；不创建无意义的 ChatGPT 研究会话。
- 非空 manifest 的全部 child run 失败时仍然按失败处理。

### TC-ADRP-10：研究 Prompt 最小化与测试隔离

1. 构造包含超长背景、原始片段、单题要求和日报 `AGENTS.md` 的研究问题。
2. 生成最终单题 Prompt，检查内容与字符数。
3. 在 `BIFROST_E2E=1` 下分别测试未配置 mock、配置 mock、显式开启 live 三种情况。

预期：

- 单题 Prompt 保留完整原始问题、必要研究要求和输出契约，但不包含整份日报 `AGENTS.md`。
- 背景、片段、单题要求与已核验上下文超过上限时截断，并明确提示以原始问题为准。
- E2E 未配置 mock 时在启动真实浏览器前失败；配置 mock 时不访问真实 ChatGPT；只有 `BIFROST_CHATGPT_WEB_LIVE_E2E=1` 才允许创建真实会话。

## 执行记录

| 日期 | 用例 | 结果 |
| --- | --- | --- |
| 2026-07-15 | TC-ADRP-10 | 根据真实信贷研究 Prompt（19594 字节）确认其中误带整份“全天候私人助理整理指南”。修复后单题 Prompt 不再读取日报 `AGENTS.md`，并为背景、原始片段、单题要求和已核验上下文设置独立字符上限；`daily_agent_research_child_prompt_is_compact_and_excludes_daily_report_instructions` 与 `live_chatgpt_web_is_fail_closed_during_e2e_without_explicit_opt_in` 通过。mock 流水线 E2E task `dd2a3454d9154d96a3bf4c41f6662fae` 通过且未创建真实 Pro 会话；`local-ci.sh --skip-e2e` 的格式、clippy、全工作区测试和依赖审计全部通过。 |
| 2026-07-15 | TC-ADRP-02/06 历史补跑与真实微信投递 | 使用 `2026-07-09` 的真实“帮我记录一下”研究清单补跑。ChatGPT Web 在 Project“日报研究”中以 Chat + Pro 分别生成 4 个独立会话；四份结果大小分别为 31808、19590、12222、12069 字节，均包含 `原始问题 / 核心结论 / 事实与证据 / 推断与不确定性 / 对原始问题的直接回答` 五段。fan-out 聚合报告 76913 字节，digest run `1784046733550-ec44d3d4-c38a-4de7-a6c0-d35cb81ac105` 成功生成 21602 字节摘要。首次自动微信投递明确失败为 `weixin sendmessage failed: ret=-2`，消息日志正确记录失败；用户在 Bot 对话发送 `1` 刷新微信会话后，补发两段摘要成功，出站日志 `ca7e24f3`、`6fccd249` 均为 `✓`。 |
| 2026-07-14 | TC-ADRP-09 | 通过。隔离数据目录和动态端口运行完整五段链路：dispatcher 输出 `{"questions":[]}`；fan-out 成功保存唯一的 `manifest.json`，未创建 child run，日期报告明确写明无外部研究问题；digest 继续生成同义摘要。原有非空 manifest 全部 child 失败负向单测仍通过。 |
| 2026-07-13 | TC-ADRP-05 长文本保存时序回归 | Playwright 用例确认 Project URL 连续输入期间不提交，按回车后只保存完整 URL；前端定向用例与构建通过。 |
| 2026-07-13 | TC-ADRP-01 当前主干回归 | `SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_asr_daily_agents_api.sh` 通过；基础四段使用外部 Codex mock，逐题研究使用 ChatGPT Web mock。五个 Agent 全部生成 processed document，两个原始问题获得不同 conversation id 和完整链接，内部故障记录未进入 manifest，fan-out 的两个链接均进入 digest 上游输入和最终 digest。 |
| 2026-07-12 | TC-ADRP-01 | `bash e2e-tests/tests/test_asr_daily_agents_api.sh` 通过；五个 Agent 均产出 processed document，两个研究问题生成不同 conversation id 与链接。 |
| 2026-07-12 | TC-ADRP-02 免费账号负向用例 | 真实 run `1783850408000-b48c2cdf-711f-40fe-9171-0e481a2c4c90` 在发送前失败，错误为 `chat_mode_control_not_found`；诊断页面确认该独立 Chrome 登录的是免费账号，Prompt 未发送。 |
| 2026-07-12 | TC-ADRP-03 状态契约 | 单元测试覆盖 `verified / unavailable / missing`；Pro 账号真实 GitHub 读取等待用户在 Bifrost 独立 Chrome 完成账号切换。 |
| 2026-07-12 | TC-ADRP-04 | 检查 `ibkr-portfolio-dashboard`：GitHub 跟踪代码、Supabase schema 和读取逻辑；真实交易行不在 Git 跟踪文件中。context Runner 类型限制测试与编译通过。 |
| 2026-07-12 | TC-ADRP-05 | 前端 TypeScript 构建与 160 个单元测试通过；临时 18883 真实服务页面确认 fan-out 开关已开启、Max Questions=8、Runner=`chatgpt-web`、`ibkr_runtime` fallback 可见，页面明确提示每题独立会话及 Chat + Pro 约束。截图：`artifacts/asr-daily-research-ui.png`。 |
| 2026-07-12 | TC-ADRP-06 | 待 Pro 真实研究完成后执行微信与周度洞察人验。 |
| 2026-07-12 | TC-ADRP-07 | 通过。使用默认 `research_seed` Prompt 真实运行 Codex：用户确认的 10 个研究主题全部进入 `research_questions`，6 个故障/执行事项全部进入 `non_research_items`（10/10、6/6）。追加未直接写入样例的泛化测试：云厂商基础设施判断、多代理框架比较、保留音色翻译正确进入研究；只有主题词的“帮我记录一下 SpaceX”进入 `memory_only`，异常归因产品灵感进入 `weekly_insight`，入口白屏修复进入 `action_item`。全程未输出优先级或分数。 |
| 2026-07-13 | TC-ADRP-08 | 通过。真实 Project“日报研究”在带 nonce 的入口保持 Chat + Pro + Project composer；ChatGPT 将裸 project id 重定向为带 `ri-bao-yan-jiu` slug 后仍被识别为同一项目。11 个既有真实研究会话已逐项迁入。新建冒烟会话 `6a549006-51a0-83ea-945a-1568e098f08f` 位于该 Project 内，Pro 最终回复 `PROJECT_ARCHIVE_OK`。Project URL 单测、研究 fan-out API E2E 和 WebUI 保存测试均通过。 |
| 2026-07-14 | TC-ADRP-01/02/03 回归 | `bifrost-admin` release 单测 2627/2627 通过；新增真实 mock CLI/CDP/HTTP 用例覆盖编排依赖链、局部/全部失败、GitHub `verified/unavailable/missing`、本地 context fallback、Chat + Pro 发送前门禁、Project 新会话恢复和选定 Agent API 异步执行。新增生产 Rust changed-lines 覆盖率为 95.57%（1727/1807），通过 95% 门禁。 |
