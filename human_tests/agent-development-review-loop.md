# Agent Development Review Loop 真实场景测试

## 功能模块说明

验证 `AGENTS.md` 中新增的开发任务强制闭环规则：Agent 必须在实现后、收尾前至少执行两轮 `目标复核 -> 代码 review -> 修复问题 -> 测试运行 -> 结果复盘`，并且相关设计文档与 `human_tests` 索引同步。

## 前置条件

1. 当前工作目录为仓库根目录：`~/work/github/bifrost`
2. 已完成本次文档修改：
   - `AGENTS.md`
   - `design/agent-development-review-loop.md`
   - `human_tests/agent-development-review-loop.md`
   - `human_tests/readme.md`

## 测试用例列表

### TC-ADRL-01：AGENTS.md 包含两轮 Review/Fix/Test 强制闭环

**操作步骤：**

1. 执行：
   ```bash
   rg -n "Review/Fix/Test 第 1 轮|Review/Fix/Test 第 2 轮|两轮|目标复核|代码 review|测试运行|结果复盘" AGENTS.md design/agent-development-review-loop.md human_tests/agent-development-review-loop.md
   ```
2. 检查输出是否同时命中 `AGENTS.md`、`design/agent-development-review-loop.md` 和 `human_tests/agent-development-review-loop.md`。
3. 执行：
   ```bash
   rg -n "规划阶段自检|闭环门禁|继续循环条件|测试完备性检查清单|最终门禁" AGENTS.md
   ```
4. 检查输出是否覆盖规划、闭环、继续循环、完备性检查和最终门禁。

**预期结果：**

- `AGENTS.md` 明确要求至少两轮 Review/Fix/Test。
- `AGENTS.md` 的规划阶段要求 TodoWrite 中必须包含两条独立闭环任务。
- `AGENTS.md` 的验证阶段包含第 1 轮、第 2 轮和继续循环条件。
- `AGENTS.md` 的收尾门禁和测试完备性检查清单都包含两轮闭环要求。
- `design/agent-development-review-loop.md` 描述该流程的设计、依赖、测试方案和校验要求。

### TC-ADRL-02：human_tests 索引同步且不维护全局总计

**操作步骤：**

1. 执行：
   ```bash
   rg -n "agent-development-review-loop|Agent Development Review Loop" human_tests/readme.md
   ```
2. 检查输出是否包含 `human_tests/agent-development-review-loop.md` 的索引行。
3. 检查索引行的测试用例数是否为 `8`。
4. 执行：
   ```bash
   ! rg -n "^\\*\\*总计：|总计：[0-9]+ 个测试文件" human_tests/readme.md
   ```
5. 检查命令是否没有命中全局测试文件数或测试用例数汇总。

**预期结果：**

- `human_tests/readme.md` 包含 `agent-development-review-loop.md` 索引。
- 索引行描述该模块用于验证 Agent 开发任务的两轮 Review/Fix/Test 闭环。
- `human_tests/readme.md` 不包含“总计：N 个测试文件，M 个测试用例”这类全局汇总数字。
- 索引维护只更新相关模块行，不要求维护全局总计。

### TC-ADRL-03：AGENTS.md 包含持续改进引导语

**操作步骤：**

1. 执行：
   ```bash
   rg -n "任何人都会犯错|反复检查|持续改进|巅峰水平|执行心法|持续改进引导" AGENTS.md design/agent-development-review-loop.md
   ```
2. 检查输出是否同时命中 `AGENTS.md` 和 `design/agent-development-review-loop.md`。

**预期结果：**

- `AGENTS.md` 顶部包含“任何人都会犯错，但我们可以通过反复检查、持续改进，终将达到巅峰水平”的引导。
- `AGENTS.md` 明确 Agent 应把 review、修复和复测视为高质量交付常态。
- `design/agent-development-review-loop.md` 同步记录该持续改进引导。

### TC-ADRL-04：AGENTS.md 包含顶层执行模型和文档变更边界

**操作步骤：**

1. 执行：
   ```bash
   rg -n "执行模式与完成定义|任务模式判定|开发模式|检查模式|CI 闭环模式|文档/流程变更模式|证据台账|完成定义|Definition of Done|不适用" AGENTS.md design/agent-development-review-loop.md
   ```
2. 检查输出是否同时命中 `AGENTS.md` 和 `design/agent-development-review-loop.md`。

**预期结果：**

- `AGENTS.md` 包含任务模式判定、证据台账和完成定义。
- `AGENTS.md` 明确检查模式只读、开发模式完整闭环、CI 闭环必须走 `github-actions-pat`。
- `AGENTS.md` 明确纯文档/流程变更的 Rust/E2E/local-ci 不适用边界，但仍要求执行 human_tests。
- `AGENTS.md` 明确任务启动前必须检查工作区状态。
- `design/agent-development-review-loop.md` 描述相同顶层执行模型。

### TC-ADRL-05：AGENTS.md 包含目标清单、diff 复核、失败归因和交付模板

**操作步骤：**

1. 执行：
   ```bash
   rg -n "用户目标验证清单|git status --short|git diff --cached|测试失败归因规则|最终交付自检模板|验证矩阵|功能缺陷|测试缺陷|环境/依赖问题|需求/文档不一致" AGENTS.md design/agent-development-review-loop.md
   ```
2. 检查输出是否覆盖规划阶段、两轮 Review/Fix/Test、测试失败归因、收尾门禁和测试完备性检查清单。

**预期结果：**

- `AGENTS.md` 要求开发前拆解用户目标验证清单。
- `AGENTS.md` 要求两轮 review 都执行 `git status --short`、`git diff` 和必要的 `git diff --cached`。
- `AGENTS.md` 要求测试失败归因为功能缺陷、测试缺陷、环境/依赖问题或需求/文档不一致。
- `AGENTS.md` 提供最终交付自检模板，包含目标对齐、Review/Fix/Test 闭环、验证矩阵、变更范围和残余风险。

### TC-ADRL-06：开发流程阶段编号连续且无重复

**操作步骤：**

1. 执行：
   ```bash
   sed -n '/### 第一阶段：分析与规划/,/## 测试覆盖要求/p' AGENTS.md | rg -n "^[0-9]+\\. \\*\\*"
   ```
2. 检查 `开发需求标准流程` 下的编号是否从 1 到 22 连续递增，无重复、跳号或回退。

**预期结果：**

- `AGENTS.md` 的开发需求标准流程编号连续，插入新步骤后没有重复编号。
- Review/Fix/Test、测试失败归因、收尾门禁和最终交付自检都处在连续流程中。

### TC-ADRL-07：任务启动前检查工作区，并行开发优先使用 worktree

**操作步骤：**

1. 执行：
   ```bash
   rg -n "任务启动与工作区隔离|git status --short --branch|git worktree|并行开发|工作区存在既有修改|豁免原因|任务启动检查" AGENTS.md design/agent-development-review-loop.md human_tests/agent-development-review-loop.md
   ```
2. 检查输出是否同时命中 `AGENTS.md`、`design/agent-development-review-loop.md` 和 `human_tests/agent-development-review-loop.md`。
3. 检查 `AGENTS.md` 是否要求任务开始前执行 `git status --short --branch`。
4. 检查 `AGENTS.md` 是否要求存在既有修改、暂存内容、未跟踪文件或并行任务时，默认优先创建或切换独立 `git worktree`。
5. 检查 `AGENTS.md` 是否要求不使用 worktree 时必须记录豁免原因。

**预期结果：**

- `AGENTS.md` 包含 `任务启动与工作区隔离` 章节。
- `AGENTS.md` 的开发流程第 1 步是 `任务启动检查`。
- `design/agent-development-review-loop.md` 同步说明并行开发和 CI 复现应优先使用独立 worktree。
- `human_tests/agent-development-review-loop.md` 包含本用例，避免后续流程文档遗漏该门禁。

### TC-ADRL-08：AGENTS.md 禁止 human_tests/readme.md 全局汇总数字

**操作步骤：**

1. 执行：
   ```bash
   rg -n "索引冲突约束|只维护测试文档索引|禁止新增或更新.*总计|只调整相关模块的索引行" AGENTS.md human_tests/readme.md
   ```
2. 执行：
   ```bash
   ! rg -n "^\\*\\*总计：|总计：[0-9]+ 个测试文件" human_tests/readme.md
   ```
3. 检查输出是否说明 `human_tests/readme.md` 只维护索引，不维护全局测试文件数或测试用例数。

**预期结果：**

- `AGENTS.md` 明确禁止在 `human_tests/readme.md` 新增或更新全局汇总数字。
- `human_tests/readme.md` 的维护约束明确不维护全局汇总数字。
- `human_tests/readme.md` 当前不包含全局总计行，避免并行开发反复冲突。

**执行记录（2026-06-09）**：PASS — 已执行索引检索、全局总计反向检索和 AGENTS/readme 约束检索；`human_tests/readme.md` 包含 `agent-development-review-loop.md` 索引行且用例数为 8，未命中 `总计：N 个测试文件` 类全局汇总行，AGENTS 与 readme 均明确禁止维护全局总计数字。

## 清理步骤

本测试仅执行文档检索命令，不启动服务，不写入临时数据目录，无需额外清理。
