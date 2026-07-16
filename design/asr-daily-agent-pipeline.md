# ASR Daily Agent Pipeline

## 背景与目标

现有 ASR Daily Agent 支持同一任务配置多个 Agent，并按配置顺序逐个执行，但每个 Agent 只消费当日转写和自己的历史输出。它缺少显式依赖、上游产物传递、依赖失败传播和循环依赖校验，因此无法可靠表达：

`日报生成 -> 研究种子抽取 -> 调研分流 -> 结果投递`

本模块把多 Agent 从“顺序列表”升级为向后兼容的轻量 DAG，同时保留现有单 Agent、默认双 Agent、手动运行、ChatGPT Web、Codex、Bifrost Agent 和 IM 投递行为。

## 用户目标验证清单

### 必须实现

- Daily Agent 可以声明一个或多个上游依赖。
- 执行顺序由依赖关系决定，不依赖 UI 数组顺序。
- 依赖产物按同一日期提供给下游 Agent。
- 文件型 Runner 通过 `input/upstream/<agent_id>/<date>-report.md` 读取上游产物。
- ChatGPT Web Runner 每次消息内直接获得上游产物正文。
- 上游失败时，下游默认跳过；用户可配置为继续执行。
- 未知依赖、自依赖、重复依赖和循环依赖在保存配置时被拒绝。
- WebUI 可以配置依赖和依赖失败策略，并看到依赖关系。
- 每个 Agent 的 Prompt 独立保存、独立编辑；研究 child run 只继承所属 fan-out Agent 的用户自定义 Prompt。
- 提供可一键套用的官方日报研究模板，但模板中只包含通用角色、最短 Prompt 和依赖关系。
- 官方模板不得包含任何真实用户的关键词、仓库名、Provider、Project URL、术语、上下文 profile 或个人 Prompt。
- 研究 fan-out 支持用户可配置的有界并发，并保持最终报告与 manifest 中的问题顺序一致。

### 必须不破坏

- 旧配置没有依赖字段时继续按原有数组顺序执行。
- `daily_report`、`tomorrow_todo` 默认行为和输出契约不变。
- 手动只运行单个 Agent 时，不隐式启动其他 Agent；依赖未运行按策略处理。
- 每个 Agent 的工作目录、指令、会话、processed state、IM 投递和报告同步继续隔离。
- 不把完整内部转写自动发送给非依赖的 Runner。

### 必须真实验证

- 至少两个 Agent 串联能在隔离数据目录产出同日上下游报告，多层 DAG 顺序由单元测试覆盖。
- ChatGPT Web 能收到上游研究种子而不是只收到原始转写。
- 公开研究种子可以交给 ChatGPT Web；内部种子留在本地 Runner。
- 最终结果可通过现有 Weixin Provider 投递。
- 周度洞察通过现有 Agent Schedule 读取最近七天产物并投递。

## 配置模型

每个 `AsrDailyAgentItem` 新增：

```yaml
dependencies:
  - agent_id: research_seed
    include_output: true
dependency_failure_policy: skip
```

- `agent_id`：当前任务中已存在的 Agent ID。
- `include_output`：默认 `true`。为 `false` 时仅建立执行顺序，不挂载产物。
- `dependency_failure_policy`：
  - `skip`：默认；任一依赖未成功时跳过当前 Agent。
  - `continue`：记录依赖问题，但仍运行当前 Agent；只注入可用产物。

旧配置反序列化为无依赖，行为不变。

## 执行逻辑

1. 读取并规范化任务内全部 Agent。
2. 校验依赖 ID、自依赖、重复依赖和 DAG 环。
3. 无依赖配置时保留原数组顺序；存在依赖时使用稳定拓扑排序，同层 Agent 保留原数组顺序。
4. 每个 Agent 运行前检查依赖结果：
   - 全部成功：继续。
   - 失败、跳过、未运行：按 `dependency_failure_policy` 处理。
5. 对 `include_output=true` 的依赖，把上游输出同步到下游工作区。
6. 下游照常生成自己的 change plan、执行 Runner、保存报告和发送 IM。

顶层 DAG 当前采用稳定拓扑顺序串行执行，不声明同层并发。原因是同一任务共享运行锁、processed state 和部分 Runner 会话状态；在这些状态完成事务化拆分前，并行执行同层 Agent 会引入覆盖与重复运行风险。研究 fan-out 是明确的并发边界：同一 manifest 中互不依赖的问题可按 `max_concurrency` 有界并发，结果完成后再按原问题顺序落盘并生成索引。因此产品对外准确表述为“串行阶段编排 + 并发研究分流”，而不是“任意 DAG 全并发”。

跳过状态使用 `skipped_dependency_failed`，错误信息包含依赖 Agent ID 和状态，便于 WebUI 与 API 诊断。

## 上游产物

上游报告来源：

`agents/<upstream_id>/output/<upstream_output_dir>/<date>-report.md`

下游稳定路径：

`agents/<downstream_id>/input/upstream/<upstream_id>/<date>-report.md`

同步使用普通文件复制并覆盖同名旧产物；只接受合法 Agent token 和 `YYYY-MM-DD-report.md`，不允许路径穿越。下游消费前还必须确认上游 processed state 的源哈希与当天最新 Daily Markdown 一致；仅存在旧 report 文件不视为依赖满足。

Prompt 规则：

- Codex、Bifrost Agent 等文件型 Runner 收到上游路径说明。
- ChatGPT Web 收到与本轮变更日期匹配的上游文件完整正文。
- 缺失或陈旧的产物由执行层按 `skip` / `continue` 策略处理。

## 用户自定义 Prompt 与工具路由

每个 Agent 使用现有 `instructions_source=default|custom` 与 `instructions` 字段独立保存 Prompt。WebUI 对每个 Agent 暴露可编辑的 Custom Prompt；更新一个 Agent 不影响同任务内其他 Agent，也不影响其他用户或任务。

研究 dispatcher 的自定义 Prompt 可以定义用户自己的领域路由，例如“出现某类术语时，把指定仓库写入 manifest 的 `github_repositories`”。这类术语与仓库映射只存在用户任务配置中，不写入内置模板。manifest 声明仓库后，ChatGPT Web child Prompt 会要求使用已连接的 GitHub Connector；Prompt 只能请求工具，不能授予仓库权限。用户仍需在自己的 ChatGPT/GitHub 连接器中完成授权与索引。

研究 child run 的 Prompt 由四层组成：

1. 系统维护的最小研究契约（直接回答、来源与输出结构）；
2. fan-out Agent 的用户自定义 Prompt，仅当 `instructions_source=custom` 时注入；
3. dispatcher 为该问题生成的 `research_prompt`；
4. 原始问题、背景、原始片段、仓库列表和可选的已核验上下文。

自定义 Prompt 单独限长并清晰标记。默认日报等大模板不得隐式注入 child run，避免把无关的长 Prompt 重复发送给 ChatGPT Pro。

## 官方模板

内置 `daily-research` 官方模板提供以下五个通用节点：

`daily_report -> research_seed -> research_dispatcher -> research_fanout -> research_digest`

- `daily_report`：从当日材料生成简短日报。
- `research_seed`：原样提取值得独立研究的问题。
- `research_dispatcher`：输出结构化 manifest，并由用户 Prompt 决定领域关键词、仓库和 Runner 路由。
- `research_fanout`：逐题独立研究，允许有界并发。
- `research_digest`：汇总核心结论与完整研究链接。

模板 API 返回可审计的模板元数据和 Agent 配置；应用模板时替换当前 Agent 列表，但保留任务级术语与报告同步目录。Runner 默认复用任务当前首个 Agent 的 Runner，也允许调用方显式覆盖主 Runner 与研究 Runner。模板默认关闭 IM 投递，不包含 Channel；用户确认目标后自行启用。模板中的全部 Prompt 必须是最短通用文本，不出现个人实例中的真实词汇、仓库、账户、Project URL 或上下文 profile。

模板应用后，各 Agent 都是普通的用户配置副本：用户可以编辑、删除、增补依赖或另存配置，不与内置模板共享可变状态。模板升级也不会覆盖已经应用到任务的用户 Prompt。

## 日报研究工作流（本地配置，不进入默认值）

### 2026-07-12 完整研究编排决策

真实日报验证后，研究链路从固定的 `daily_report -> research_agent` 两段升级为：

`daily_report -> research_seed -> research_dispatcher -> N 个独立 research child run -> research_digest`

- `research_seed` 必须保留每一项的 `original_question`、原始片段、提出问题时的背景和所需上下文，不能只输出缩写标题或优先级分数。
- `research_dispatcher` 是主编排 Agent：只负责识别问题类型、选择 Runner、选择上下文配置并生成结构化 manifest，不在同一会话里完成全部研究。
- 每个研究问题创建独立 child run；ChatGPT Web 每题创建新 conversation，Codex/Bifrost Agent 每题使用独立 session key。一个问题失败不阻塞其他问题。
- `research_digest` 不再按价值/紧迫性给问题打分，而是逐项展示原始问题、核心结论、证据、不确定性和完整研究链接。
- 每个 Agent 继续支持独立 instructions；child run 的最终 Prompt 由 Agent instructions、领域模板、原始问题和上下文共同组成。
- 本轮不实现 `external_shareable` 或信息分级；相关数据边界另行设计。

上下文获取默认与研究 Agent 合并。ChatGPT Web 优先通过用户已经授权的 GitHub Connector 直接读取已提交、已授权并完成索引的仓库内容；文件型本地 Runner 则可以在显式允许的工作目录内直接读取仓库。两种情况都不额外创建“事实 Agent”。仅在以下情况拆出 context run：

1. 问题需要的事实不在 GitHub 中，例如本机未提交文件、Supabase/IBKR 运行时交易行、私有日志或实时接口结果；
2. ChatGPT Web 当前账号尚未授权目标仓库，或目标仓库尚未完成索引；
3. 同一事实包会被多个问题复用；
4. 需要保存可审计的数据快照或固定查询口径。

投资类研究默认先让 ChatGPT Web 通过 GitHub Connector 读取 `ibkr-portfolio-dashboard` 的实现、数据口径和文档。该仓库中的 Supabase 表结构和查询逻辑可直接研究，但真实成交、持仓、成本和现金记录不是 GitHub 仓库内容；涉及这些真实记录时，再由受控本地 context profile 查询并把事实包交给对应的独立 ChatGPT conversation。

ChatGPT Web child run 发送前必须强制校验：

- 使用 `Chat` 模式，不允许落到 `Work`；
- 使用 `Pro` 模型；
- 如果 fan-out 配置了 `chatgpt_project_url`，新会话必须从该 ChatGPT Project 首页创建，使会话自动归档到项目；每个问题仍创建独立 conversation，不复用其他问题的上下文。
- 模式或模型无法切换、无法验证时直接失败，不发送研究问题；
- Prompt 明确指定要使用已连接的 GitHub 仓库，并要求在结果中列出实际读取的仓库文件。若模型报告仓库不可见，则明确标记不可用；需要本地事实的问题必须预先配置 context profile，不能假装已经读取。
- 单题结果记录 `github_connector_status=verified/unavailable/missing`；只有明确返回 `verified` 才显示为 GitHub 已核验。配置了本地 context profile 时可显示 `success_with_local_context`，否则明确显示 Connector 不可用或未验证。

### ChatGPT Project 归档

`research_fanout` 可选配置 `chatgpt_project_url`，例如：

```json
{
  "chatgpt_interface_mode": "chat",
  "chatgpt_model": "pro",
  "chatgpt_project_url": "https://chatgpt.com/g/g-p-<project-id>/project"
}
```

- 保存配置时去除首尾空白并校验为 `https://chatgpt.com/g/g-p-*/project`；空值等价于未配置。
- ChatGPT 可能把无标题的 Project URL 重定向为带可读 slug 的 URL；校验以稳定 project id 为准，因此该重定向仍视为同一个项目。
- fan-out 把该值投影到单题 ChatGPT Web request 的 `chatgpt.projectUrl`，不修改 Runner 持久配置，因此普通聊天、日报 Agent 和其他任务不受影响。
- ChatGPT Web adapter 只在创建新 conversation 时导航到 Project URL；已有 `conversation_id` 的后续轮次仍导航到标准 `/c/<conversation_id>`。
- Project URL 只承担归档入口，不改变单题 Prompt。研究 Prompt 必须继续携带完整原始问题，并声明本会话只研究一个问题，不能把项目内其他聊天当作证据。
- 未配置项目时继续导航到 ChatGPT 首页，保持旧配置和旧任务行为不变。
- 项目页面缺少 composer、Chat/Pro 控件或跳到登录/真人验证时，在写入 Prompt 前失败；不静默退回普通聊天首页，避免研究会话散落到项目外。
- ChatGPT Web 没有公开、稳定的 Project 管理 API；实现继续使用现有浏览器/CDP 自动化。项目 URL 与页面控件变化由定向单元测试、E2E 和真实 human test 共同守护。

### Research manifest 契约

`research_dispatcher` 输出 Markdown 中的 JSON manifest：

```json
{
  "questions": [
    {
      "id": "2026-06-26-msft-355",
      "original_question": "微软 355 美元到底意味着什么，还有哪些下行情景？",
      "source_excerpt": "日报中的原始问题和相邻上下文",
      "background": "为什么当天提出该问题",
      "runner": "chatgpt-web",
      "github_repositories": ["ibkr-portfolio-dashboard"],
      "context_profile": "ibkr-runtime-fallback",
      "research_prompt": "直接回答原始问题，并区分事实、推断和不确定性"
    }
  ]
}
```

- `id` 仅允许稳定 token，作为目录名和 child session 后缀。
- `original_question` 必填，并原样进入研究报告与微信摘要。
- `runner` 必须在 fan-out Agent 配置的 allowlist 中，不能由模型任意指定本机命令。
- `github_repositories` 用于告诉 ChatGPT Web 应读取哪些已连接仓库；它不是授权机制，真正可见性仍由 ChatGPT Connector 授权与索引状态决定。
- `context_profile` 是可选回退，只能引用本机配置的安全映射；manifest 不能直接提供任意绝对路径。
- `research_prompt` 是单题补充要求，不替代 Agent 的基础 instructions。
- `questions: []` 是合法结果，表示当日日报没有需要外部研究的问题；fan-out 必须成功收敛，不能把零个子任务判定为全部失败。
- `max_concurrency` 只控制同一日期内 research child run 的并发上限；每个 child 使用独立 session key、结果文件和 metadata 文件。完成顺序不得改变 manifest 和汇总报告中的问题顺序。

### Child run 产物

每个日期的 fan-out 输出结构为：

```text
agents/research_dispatcher/output/research_result/
├── YYYY-MM-DD-report.md
└── YYYY-MM-DD/
    ├── manifest.json
    ├── <question_id>.md
    └── <question_id>.json
```

单题 JSON 保存 `original_question / runner / github_repositories / context_profile / status / run_id / conversation_id / full_report_link / result_path / error`。ChatGPT Web 成功时 `full_report_link` 为对应的 `https://chatgpt.com/c/<conversation_id>`；本地 Runner 使用本地结果路径。日期级 report 供下游 `research_digest` 消费，并保留每一项原始问题和完整结果链接。

当 manifest 为空时，fan-out 仍保存空的 `manifest.json` 和日期级 report，不启动任何 child run；report 明确写明当日没有外部研究问题，供 `research_digest` 和 IM 投递正常消费。“全部子任务失败”只适用于非空 manifest 且成功数为零的情况。

### daily_report

- Runner：ChatGPT Web。
- 输入：当日转写。
- 输出：结构化日报。
- IM：发送日报摘要。

### research_seed

- Runner：Codex。
- 依赖：`daily_report`。
- 先把用户主动记录事项分为 `memory_only / action_item / internal_investigation / external_research / weekly_insight`；“帮我记录一下”只保证保留，不能单独触发研究。
- 只有存在外部知识缺口、预期产物是事实/解释/比较/证据，并且主要依赖公开资料的问题，才能进入 `external_research`。
- 当前系统 Bug、日志/Trace、告警、环境差异、发布验证和明确执行事项分别进入 `internal_investigation` 或 `action_item`；产品机会和方法论进入 `weekly_insight`。
- 输出：`research_questions` 保留 `original_question/source_excerpt/background/intent_evidence/expected_evidence`；`non_research_items` 保留未派发事项及原因。不做优先级评分。
- IM：关闭。

### research_dispatcher

- Runner：Codex。
- 依赖：`research_seed`。
- 只读取 `research_questions`，绝不调度 `non_research_items`；为每题选择 Runner、GitHub 仓库和可选运行时数据 fallback，输出结构化 research manifest。
- 本轮不实现信息分级或 `external_shareable`；需要真实运行时数据的问题由明确配置的 context profile 处理。
- 输出：研究 manifest，不在调度会话里完成研究。
- IM：关闭；最终由 `research_digest` 发送各题核心结论和完整 ChatGPT 链接。

### weekly_insight

使用现有 IM Agent Schedule，不新增 Daily Agent 特殊逻辑。Codex 读取最近七天 `report/research_seed/research_result`，输出反复判断、未解决问题、形成中的方法论、产品机会、认知变化和下周优先项。

## 本轮运行边界

- 本轮暂不实现 `external_shareable` 或数据分级，后续单独设计。
- Provider ID、登录态、内部术语、真实转写和个人 Prompt 只保存在本机配置，不提交仓库。
- ChatGPT Web 使用现有独立浏览器 profile，不复制 Cookie 或密码。

## 测试方案

### 单元测试

- 无依赖保持数组顺序。
- 稳定拓扑排序、菱形依赖和多层依赖。
- 未知、自、重复、循环依赖拒绝。
- `skip` 与 `continue` 失败策略。
- 上游产物只复制合法同日报告。
- 旧 report 的 processed source hash 与当天源文不一致时判定为 stale。
- ChatGPT Web Prompt 注入同日上游正文。
- 文件型 Runner Prompt 只暴露稳定相对路径。
- 旧配置序列化/反序列化兼容。
- `chatgpt_project_url` 规范化、非法 host/path 拒绝、未配置兼容，以及 fan-out 到 adapter request 的投影。
- 新 conversation 使用 Project URL，已有 conversation 继续使用 `/c/<conversation_id>`。
- 只有 fan-out Agent 的 custom Prompt 会进入研究 child Prompt；default Prompt 不会泄漏或重复发送。
- fan-out 并发数边界校验、并发执行和结果顺序稳定。
- 官方模板 DAG 合法、Prompt 可独立修改，并且不含个人关键词、仓库、Provider、Project URL 或 context profile。

### E2E

- 使用隔离 `BIFROST_DATA_DIR`、fake external runners 和反向排列的双 Agent 配置。
- 验证 Agent 执行顺序、同日上游产物、失败跳过和 IM 内容。
- 验证手动单 Agent 运行不意外启动依赖。
- 验证项目配置进入 ChatGPT Web child request，未配置时不出现 `projectUrl`。
- 列出并一键应用官方模板，验证五节点 DAG、最短 Prompt、Runner 继承和任务级设置保留。
- 应用模板后更新单个 Agent Prompt，验证其他 Agent 与内置模板响应保持不变。

### 真实场景测试

更新 `human_tests/asr-daily-agents.md` 或新增 `human_tests/asr-daily-agent-pipeline.md`，覆盖：

- WebUI 配置依赖。
- 真实 ChatGPT Web 日报和上游注入。
- ChatGPT Web 日报中的研究种子抽取与下游调研。
- 真实 Project 首页处于 Chat + Pro，带 nonce 打开后仍显示项目 composer；发送冒烟问题后生成的独立会话出现在项目列表。
- Weixin 投递。
- 周度 Schedule 聚合。
- 登录失效、上游失败和敏感种子不外发。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核配置兼容、拓扑算法、状态一致性、目录安全和 ChatGPT Web 信息边界。
- 执行相关 Rust 单测、Web lint/typecheck、E2E 和真实场景最小链路。
- 修复发现后复跑失败路径。

### 第 2 轮

- 基于最新 diff 复核 API、WebUI、设计文档、human_tests 与默认配置一致性。
- 重跑受影响单测、E2E、human_tests 和 workspace all-features。
- 若仍有问题继续追加轮次。

### 第 3 轮

- 真实 ChatGPT Web 验证发现账号选择页在已捕获认证流量时会被误判为登录完成。
- 登录完成条件增加“账号选择器消失且 composer 可见、可编辑”，并覆盖中英文页面文案。
- 重跑登录判定单测、Daily Agent 49 项定向测试、API E2E、Web lint/build、严格 clippy 与 workspace all-features。

## 校验与文档

- `cargo test -p bifrost-admin daily_agent`
- Web lint/typecheck/build（按仓库脚本）。
- 对应 shell E2E。
- `rust-project-validate`。
- `cargo test --workspace --all-features`。
- 按改动范围执行 `scripts/ci/local-ci.sh`。
- 更新 `design/asr-daily-agent-runner.md`、`human_tests/readme.md` 和相关 README/API 文案。
