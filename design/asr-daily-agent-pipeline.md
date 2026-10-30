# ASR Daily Agent Pipeline

## 背景与目标

现有 ASR Daily Agent 支持同一任务配置多个 Agent，并按配置顺序逐个执行，但每个 Agent 只消费当日转写和自己的历史输出。它缺少显式依赖、上游产物传递、依赖失败传播和循环依赖校验，因此无法可靠表达：

`日报生成 -> 研究种子抽取 -> 调研分流 -> 结果投递`

本模块把多 Agent 从“顺序列表”升级为向后兼容的轻量 DAG，同时保留现有单 Agent、默认双 Agent、手动运行、Codex、Bifrost Agent 和 IM 投递行为。

## 用户目标验证清单

### 必须实现

- Daily Agent 可以声明一个或多个上游依赖。
- 执行顺序由依赖关系决定，不依赖 UI 数组顺序。
- 依赖产物按同一日期提供给下游 Agent。
- ASR 自动触发只携带本轮实际产出可用转写的录音日期；工作区里历史未处理输入不能被顺带加入本轮。
- 文件型 Runner 通过 `input/upstream/<agent_id>/<date>-report.md` 读取上游产物。
- 上游失败时，下游默认跳过；用户可配置为继续执行。
- 未知依赖、自依赖、重复依赖和循环依赖在保存配置时被拒绝。
- WebUI 可以配置依赖和依赖失败策略，并看到依赖关系。

### 必须不破坏

- 旧配置没有依赖字段时继续按原有数组顺序执行。
- `daily_report`、`tomorrow_todo` 默认行为和输出契约不变。
- 手动只运行单个 Agent 时，不隐式启动其他 Agent；依赖未运行按策略处理。
- 每个 Agent 的工作目录、指令、会话、processed state、IM 投递和报告同步继续隔离。
- 不把完整内部转写自动发送给非依赖的 Runner。

### 必须真实验证

- 至少两个 Agent 串联能在隔离数据目录产出同日上下游报告，多层 DAG 顺序由单元测试覆盖。
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
6. ASR completion hook 从本轮 attempted file 中筛出已成功或部分成功且确有文本产物的录音日期，去重后逐日调用整条 DAG；失败文件和未参与本轮的历史文件不提供日期。
7. 下游照常为当前日期生成 change plan、执行 Runner、保存报告和发送 IM。

跳过状态使用 `skipped_dependency_failed`，错误信息包含依赖 Agent ID 和状态，便于 WebUI 与 API 诊断。

## 上游产物

上游报告来源：

`agents/<upstream_id>/output/<upstream_output_dir>/<date>-report.md`

下游稳定路径：

`agents/<downstream_id>/input/upstream/<upstream_id>/<date>-report.md`

同步使用普通文件复制并覆盖同名旧产物；只接受合法 Agent token 和 `YYYY-MM-DD-report.md`，不允许路径穿越。下游消费前还必须确认上游 processed state 的源哈希与当天最新 Daily Markdown 一致；仅存在旧 report 文件不视为依赖满足。

Prompt 规则：

- Codex、Bifrost Agent 等文件型 Runner 收到上游路径说明。
- 缺失或陈旧的产物由执行层按 `skip` / `continue` 策略处理。
- 依赖同步目录可以保留其他日期的历史文件，但 change plan 与 Runner entry 必须始终按 completion hook 的目标日期过滤；不能因为历史 fan-out 没有 processed state 就重启六月研究。

## 日报研究工作流（本地配置，不进入默认值）

### 2026-07-12 完整研究编排决策

真实日报验证后，研究链路从固定的 `daily_report -> research_agent` 两段升级为：

`daily_report -> research_seed -> research_dispatcher -> N 个独立 research child run -> research_digest`

- `research_seed` 必须保留每一项的 `original_question`、原始片段、提出问题时的背景和所需上下文，不能只输出缩写标题或优先级分数。
- `research_dispatcher` 是主编排 Agent：只负责识别问题类型、选择 Runner、选择上下文配置并生成结构化 manifest，不在同一会话里完成全部研究。
- `research_digest` 不再按价值/紧迫性给问题打分，而是逐项展示原始问题、核心结论、证据、不确定性和完整研究链接。
- 每个 Agent 继续支持独立 instructions；child run 的最终 Prompt 由 Agent instructions、领域模板、原始问题和上下文共同组成。
- 本轮不实现 `external_shareable` 或信息分级；相关数据边界另行设计。


上下文仅使用 Runner 已明确授权的工具和目录。以下情况可以拆出独立 context run：

1. 问题需要未提交文件、私有日志或实时接口等仓库外事实；
2. 同一事实包需要被多个问题复用；
3. 需要保存可审计的数据快照或固定查询口径。

- Prompt 明确指定要使用已连接的 GitHub 仓库，并要求在结果中列出实际读取的仓库文件。若模型报告仓库不可见，则明确标记不可用；需要本地事实的问题必须预先配置 context profile，不能假装已经读取。
- 单题结果记录 `github_connector_status=verified/unavailable/missing`；只有明确返回 `verified` 才显示为 GitHub 已核验。配置了本地 context profile 时可显示 `success_with_local_context`，否则明确显示 Connector 不可用或未验证。

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
      "runner": "Codex",
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
- `context_profile` 是可选回退，只能引用本机配置的安全映射；manifest 不能直接提供任意绝对路径。
- `research_prompt` 是单题补充要求，不替代 Agent 的基础 instructions。
- `questions: []` 是合法结果，表示当日日报没有需要外部研究的问题；fan-out 必须成功收敛，不能把零个子任务判定为全部失败。

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


当 manifest 为空时，fan-out 仍保存空的 `manifest.json` 和日期级 report，不启动任何 child run；report 明确写明当日没有外部研究问题，供 `research_digest` 和 IM 投递正常消费。“全部子任务失败”只适用于非空 manifest 且成功数为零的情况。

### daily_report

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

### weekly_insight

使用现有 IM Agent Schedule，不新增 Daily Agent 特殊逻辑。Codex 读取最近七天 `report/research_seed/research_result`，输出反复判断、未解决问题、形成中的方法论、产品机会、认知变化和下周优先项。

## 本轮运行边界

- 本轮暂不实现 `external_shareable` 或数据分级，后续单独设计。
- Provider ID、登录态、内部术语、真实转写和个人 Prompt 只保存在本机配置，不提交仓库。

## 测试方案

### 单元测试

- 无依赖保持数组顺序。
- 稳定拓扑排序、菱形依赖和多层依赖。
- 未知、自、重复、循环依赖拒绝。
- `skip` 与 `continue` 失败策略。
- 上游产物只复制合法同日报告。
- 旧 report 的 processed source hash 与当天源文不一致时判定为 stale。
- 文件型 Runner Prompt 只暴露稳定相对路径。
- 旧配置序列化/反序列化兼容。
- 新 conversation 使用 Project URL，已有 conversation 继续使用 `/c/<conversation_id>`。

### E2E

- 使用隔离 `BIFROST_DATA_DIR`、fake external runners 和反向排列的双 Agent 配置。
- 验证 Agent 执行顺序、同日上游产物、失败跳过和 IM 内容。
- 验证手动单 Agent 运行不意外启动依赖。

### 真实场景测试

更新 `human_tests/asr-daily-agents.md` 或新增 `human_tests/asr-daily-agent-pipeline.md`，覆盖：

- WebUI 配置依赖。
- 真实 Project 首页处于 Chat + Pro，带 nonce 打开后仍显示项目 composer；发送冒烟问题后生成的独立会话出现在项目列表。
- Weixin 投递。
- 周度 Schedule 聚合。
- 登录失效、上游失败和敏感种子不外发。

## Review / Fix / Test 闭环

### 第 1 轮

- 复核配置兼容、拓扑算法、状态一致性、目录安全 信息边界。
- 执行相关 Rust 单测、Web lint/typecheck、E2E 和真实场景最小链路。
- 修复发现后复跑失败路径。

### 第 2 轮

- 基于最新 diff 复核 API、WebUI、设计文档、human_tests 与默认配置一致性。
- 重跑受影响单测、E2E、human_tests 和 workspace all-features。
- 若仍有问题继续追加轮次。

### 第 3 轮

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
