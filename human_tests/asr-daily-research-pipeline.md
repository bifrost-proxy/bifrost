# ASR Daily Research Pipeline

## 目标


## 用例

### TC-ADRP-01：五段 Agent 链路与逐题独立会话

1. 创建 `daily_report -> research_seed -> research_dispatcher -> research_fanout -> research_digest` 五个 Agent。
2. Dispatcher 输出两个包含 `original_question` 的问题，其中一个指定 `ibkr-portfolio-dashboard`。
3. 运行同一天的完整 Daily Agent 链路。

预期：

- 每题目录包含 manifest、Markdown 报告和 JSON 元数据。
- fan-out 汇总及 digest 上游输入保留原始问题和两个完整链接。
- 一个问题失败不阻止另一个问题生成报告。

### TC-ADRP-03：GitHub Connector 真实仓库核验

1. 在 Pro 账号授权并索引 `ibkr-portfolio-dashboard`。

预期：

- 报告包含 `GITHUB_CONNECTOR_STATUS: verified`。
- 文件路径确实存在于仓库，结论和源码一致。
- 如果仓库不可见，必须返回 `unavailable`；缺少状态标记时元数据为 `missing`，不能显示为已核验。

### TC-ADRP-04：GitHub 与真实交易数据边界

1. 研究仓库代码、表结构和查询口径。
2. 再研究用户真实成交、持仓、成本或现金记录。

预期：

- 未提交到 GitHub 的 Supabase/IBKR 真实数据不会被声称为仓库内容。

### TC-ADRP-05：配置页面

1. 打开某个 Daily Agent 的详情。
2. 开启 Independent Research。
3. 配置最大问题数、允许的研究 Runner 和可选 Runtime Data Fallback。

预期：

- 新增 fallback 必须先填写名称、Runner 和本地目录，未完整填写时不会保存无效配置。
- 长文本字段只在失焦或按回车时保存一次，不会逐字符发请求或被乱序响应覆盖。
- 每个 Agent 仍可独立编辑自己的 AGENTS.md Prompt。

### TC-ADRP-06：微信摘要与周度洞察

1. Daily report 使用摘要投递。
3. 周度 Schedule 读取最近七天的日报、研究种子和研究结果。

预期：

- 微信消息不发送七篇完整流水账。
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

### TC-ADRP-09：空研究清单正常收敛

1. 使用只包含内部故障或执行事项、没有外部研究问题的日报运行五段 Agent 链路。
2. 确认 `research_dispatcher` 输出 `{"questions":[]}`。
3. 检查 fan-out 产物和下游 `research_digest`。

预期：

- fan-out 成功完成，保存空的 `manifest.json`，且不创建任何 child run。
- 日期级 fan-out report 明确写明当日没有需要外部研究的问题。
- 非空 manifest 的全部 child run 失败时仍然按失败处理。

### TC-ADRP-10：研究 Prompt 最小化与测试隔离

1. 构造包含超长背景、原始片段、单题要求和日报 `AGENTS.md` 的研究问题。
2. 生成最终单题 Prompt，检查内容与字符数。
3. 在 `BIFROST_E2E=1` 下分别测试未配置 mock、配置 mock、显式开启 live 三种情况。

预期：

- 单题 Prompt 保留完整原始问题、必要研究要求和输出契约，但不包含整份日报 `AGENTS.md`。
- 背景、片段、单题要求与已核验上下文超过上限时截断，并明确提示以原始问题为准。

## 执行记录

| 日期 | 用例 | 结果 |
| --- | --- | --- |
| 2026-07-14 | TC-ADRP-09 | 通过。隔离数据目录和动态端口运行完整五段链路：dispatcher 输出 `{"questions":[]}`；fan-out 成功保存唯一的 `manifest.json`，未创建 child run，日期报告明确写明无外部研究问题；digest 继续生成同义摘要。原有非空 manifest 全部 child 失败负向单测仍通过。 |
| 2026-07-12 | TC-ADRP-01 | `bash e2e-tests/tests/test_asr_daily_agents_api.sh` 通过；五个 Agent 均产出 processed document，两个研究问题生成不同 conversation id 与链接。 |
| 2026-07-12 | TC-ADRP-02 免费账号负向用例 | 真实 run `1783850408000-b48c2cdf-711f-40fe-9171-0e481a2c4c90` 在发送前失败，错误为 `chat_mode_control_not_found`；诊断页面确认该独立 Chrome 登录的是免费账号，Prompt 未发送。 |
| 2026-07-12 | TC-ADRP-03 状态契约 | 单元测试覆盖 `verified / unavailable / missing`；Pro 账号真实 GitHub 读取等待用户在 Bifrost 独立 Chrome 完成账号切换。 |
| 2026-07-12 | TC-ADRP-04 | 检查 `ibkr-portfolio-dashboard`：GitHub 跟踪代码、Supabase schema 和读取逻辑；真实交易行不在 Git 跟踪文件中。context Runner 类型限制测试与编译通过。 |
| 2026-07-12 | TC-ADRP-06 | 待 Pro 真实研究完成后执行微信与周度洞察人验。 |
| 2026-07-12 | TC-ADRP-07 | 通过。使用默认 `research_seed` Prompt 真实运行 Codex：用户确认的 10 个研究主题全部进入 `research_questions`，6 个故障/执行事项全部进入 `non_research_items`（10/10、6/6）。追加未直接写入样例的泛化测试：云厂商基础设施判断、多代理框架比较、保留音色翻译正确进入研究；只有主题词的“帮我记录一下 SpaceX”进入 `memory_only`，异常归因产品灵感进入 `weekly_insight`，入口白屏修复进入 `action_item`。全程未输出优先级或分数。 |
