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

## 日报研究工作流（本地配置，不进入默认值）

### daily_report

- Runner：ChatGPT Web。
- 输入：当日转写。
- 输出：结构化日报。
- IM：发送完整日报。

### research_seed

- Runner：Codex。
- 依赖：`daily_report`。
- 输出：Markdown 中嵌 JSON 研究种子；至少包含 `id/question/type/priority/external_shareable/status`。
- IM：关闭。

### research_dispatcher

- Runner：Codex。
- 依赖：`research_seed`。
- 公开且 `external_shareable=true` 的问题调用 ChatGPT Web Runner。
- `internal_repo`、`internal_log` 和未脱敏 `mixed` 问题留在本地 Runner。
- 输出：研究结果和逐项状态。
- IM：发送完整结果。

### weekly_insight

使用现有 IM Agent Schedule，不新增 Daily Agent 特殊逻辑。Codex 读取最近七天 `report/research_seed/research_result`，输出反复判断、未解决问题、形成中的方法论、产品机会、认知变化和下周优先项。

## 安全边界

- ChatGPT Web 只接收显式依赖产物和公开研究所需的最小上下文。
- 研究种子必须显式标记 `external_shareable`；默认 `false`。
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

### E2E

- 使用隔离 `BIFROST_DATA_DIR`、fake external runners 和反向排列的双 Agent 配置。
- 验证 Agent 执行顺序、同日上游产物、失败跳过和 IM 内容。
- 验证手动单 Agent 运行不意外启动依赖。

### 真实场景测试

更新 `human_tests/asr-daily-agents.md` 或新增 `human_tests/asr-daily-agent-pipeline.md`，覆盖：

- WebUI 配置依赖。
- 真实 ChatGPT Web 日报和上游注入。
- ChatGPT Web 日报中的研究种子抽取与下游调研。
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
