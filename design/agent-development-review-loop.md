# Agent Development Review Loop

## 背景

Bifrost 仓库同时承载 CLI、Web/Admin、IM Gateway、Agent、Runner、Sync、Remote Invoke、Skills 等模块，交付路径长（本地实现 → 单元测试 → E2E → human_tests → 提交 → 推送 → MR/PR → 远端 CI → 合并）。Agent 在此仓库执行开发任务时常见的失败模式：

- 只完成了实现，没有对齐用户目标验证清单；
- 一轮 review 就结束，没有第二轮复查修复；
- 测试失败被“绕过”而不是“归因修复”；
- 只跑单测，没跑 E2E / human_tests；
- 修复完文档没同步 `human_tests/readme.md` 索引；
- 只 push 本地就交付，不看远端 CI；
- 并行开发时把脏工作区噪声当成真实失败。

本模块把 Agent 的开发闭环规范化为一个可执行状态机：`目标复核 → 代码 review → 修复 → 测试运行 → 结果复盘`，至少执行两轮，直到用户目标验证清单全部落地。设计文档、`AGENTS.md`、`human_tests/agent-development-review-loop.md` 三处同步。

## 用户目标验证清单

### 必须实现

- `AGENTS.md` 顶部包含持续改进引导语，把 review、修复、复测定义为高质量交付的常态。
- `AGENTS.md` 与本设计文档明确任务模式判定：开发模式、检查模式、CI 闭环模式、文档/流程变更模式。
- 任务启动前必须执行 `git status --short --branch`；已有修改或存在并行开发时默认使用独立 `git worktree`。
- 每个开发任务必须写出用户目标验证清单：`必须实现` / `必须不破坏` / `必须真实验证` / `必须交付`。
- 强制执行至少两轮 `目标复核 → 代码 review → 修复 → 测试运行 → 结果复盘` 闭环。
- 任一轮出现新 P0/P1/P2 缺陷、测试未覆盖关键路径、human_tests 缺失、CLI/WebUI/API/协议不一致或用户目标未闭合时，必须继续追加新一轮。
- 默认执行完整交付闭环：本地关键验证通过后提交、推送任务分支、创建或更新 draft MR/PR、fail-fast 看护远端 CI。
- 最终交付回复必须包含分支、commit、MR/PR 链接、CI run id 与状态、变更范围、残余风险。

### 必须不破坏

- 不改变现有 `human_tests/` 目录结构与 `human_tests/readme.md` 索引维护规则；禁止在 readme 里维护全局测试文件数或用例数总计。
- 不改变 CLI / Admin API / WebUI 已有契约；本设计只规范 Agent 行为。
- 不打断已有 CI wrapper 与 local-ci 脚本的调用顺序。
- 不与 `design/agent-plan-lifecycle.md`、`design/agent-loop-process-isolation.md` 中的 runtime 语义冲突。
- 不使用 `--no-verify` 或类似跳过 hook 的手段。
- 不通过“删除失败断言 / 缩小用例 / 把未验证说成通过”来获得绿灯。

### 必须真实验证

- 本轮变更为文档 / 流程 / 说明变更，无 Rust、WebUI、脚本、配置、协议或运行行为改动时，可标记 Rust 单元、E2E、workspace all-features、local-ci 为不适用；但必须逐条真实执行 `human_tests/agent-development-review-loop.md`。
- 一旦本轮涉及代码变更，必须按 `AGENTS.md` 执行完整校验（fmt / clippy / 相关单元 / E2E / workspace / human_tests / rust-project-validate）。
- 提交后必须查看远端 CI run id 与状态；CI 失败必须进入 fix → push → watch 循环，直到全绿或有明确外部阻塞证据。

### 必须交付

- 更新本设计文档、`AGENTS.md`、`human_tests/agent-development-review-loop.md`、`human_tests/readme.md`。
- 完成至少两轮 Review/Fix/Test 闭环并在最终交付回复中列出证据。

## 产品语义

### 持续改进引导

`AGENTS.md` 顶部与本设计文档均包含：

> 任何人都会犯错，但通过反复检查、持续改进，终将达到巅峰水平。

用于把 review、修复和复测定义为高质量交付的常态，而不是额外负担。

### 顶层执行模型

`AGENTS.md` 把开发手册组织成可执行状态机而不是规则集合：

- **任务模式判定**：开发模式、检查模式、CI 闭环模式、文档/流程变更模式。
- **任务启动与工作区隔离**：每个任务先检查 `git status --short --branch`；若已有修改或存在并行开发，默认优先使用独立 `git worktree`。
- **证据台账**：目标证据、工作区隔离证据、变更证据、测试证据、review 证据、边界证据。
- **默认交付闭环**：除非用户明确豁免，开发任务完成后必须提交、推送、创建或更新 MR/PR，并跟进远端 CI 到全绿。
- **完成定义**：目标、代码、测试、文档、human_tests、review 闭环、提交/MR/CI 看护和最终交付全部有证据。

### 默认提交、MR 与 CI 看护

Agent 对任何修改仓库文件的开发任务都应默认执行完整交付闭环，不需要用户在实现后再次提醒：

- 本地关键验证通过后提交并推送当前任务分支。若完整验证耗时长，可先完成格式、clippy 或相关最小测试后提前推送，让远端 CI 与剩余本地验证并行。
- 确认当前分支有对应 MR/PR：无则创建 draft MR/PR，有则更新同一 MR/PR。
- 推送后加载 `.agents/skills/github-actions-pat/`，获取 run id 并使用 fail-fast 看护远端 CI。
- CI 失败时按日志归因后进入 fix → push → watch 循环，直到全绿；只有外部阻塞、无关失败、权限/token 不足或用户明确叫停时，才可停止并带证据交付。
- 最终回复必须包含分支、commit、MR/PR 链接、CI run id 和状态。

### 并行开发与 worktree 隔离

任务开始前必须确认当前工作区是否干净：

- 干净工作区可以直接继续。
- 存在既有修改、暂存内容、未跟踪文件或并行任务时，默认优先创建或切换到独立 `git worktree` 再开发。
- 只有用户明确要求在当前工作区继续，或本次任务就是处理这些既有修改时，才允许豁免 worktree；豁免原因必须进入最终交付的变更范围说明。
- CI 复现、长任务、多 Agent 并行开发必须使用独立 worktree，避免把脏工作区噪声误判为真实失败。

### 双轮闭环

所有开发任务在实现后进入强制闭环阶段，至少执行两轮：

1. **第 1 轮：实现后自查闭环**
   - 重新读取用户目标验证清单、相关设计文档、修改过的文件和测试计划。
   - 执行 `git status --short`、`git diff` 和必要的 `git diff --cached`，确认修改文件、未跟踪文件、暂存文件和用户既有改动边界。
   - 执行代码 review，按严重程度列出问题、风险、遗漏测试和文档缺口。
   - 立即修复发现的问题。
   - 运行与本轮修复直接相关的最小验证；测试失败必须先归因再修复。
   - 记录本轮结果：发现的问题、修复摘要、测试命令和结果。

2. **第 2 轮：修复后复查闭环**
   - 重新对照用户目标和第 1 轮问题清单，确认没有只修测试、不修功能的问题。
   - 再次执行 `git status --short`、`git diff` 和必要的 `git diff --cached`，以修复后的最新 diff 为准复查。
   - 再次 review 修改文件、测试覆盖、human_tests 和文档同步状态。
   - 修复新发现的问题。
   - 重新运行受影响测试；如果有修复，必须复跑对应失败路径。
   - 记录本轮结果。

### 用户目标验证清单模板

开发前必须把用户目标拆成可验证清单，并在两轮闭环和最终交付中逐条对齐：

- **必须实现**：用户明确要求的新能力、修复点、行为变化。
- **必须不破坏**：相关旧行为、兼容性、权限边界、数据安全、性能边界。
- **必须真实验证**：真实 CLI/WebUI/API、真实链路、真实 CI、远端设备或用户明确要求的运行路径。
- **必须交付**：文档、测试、human_tests、提交、推送、CI 看护等交付动作。

### 继续循环条件

两轮是最低要求，不是上限。任一轮出现以下情况时，必须继续追加新一轮，直到没有阻塞问题：

- 发现新的 P0/P1/P2 功能缺陷、安全风险、数据损坏风险或用户可感知回归。
- 测试失败、测试未覆盖关键路径，或只验证了 mock 路径而用户要求真实链路。
- human_tests 文档、索引或真实执行结果缺失。
- 文档、CLI help、WebUI、API、协议或配置默认值之间不一致。
- 用户目标仍有未验证或未实现条目。

### 测试失败归因

任何测试失败都必须先归因再修复，禁止为了绿灯直接删除断言、缩小用例或把未验证说成通过：

- **功能缺陷**：修功能，复跑失败路径和相关回归。
- **测试缺陷**：说明产品行为正确的证据，修测试并保留有效断言。
- **环境/依赖问题**：记录端口、网络、权限、外部服务或 CI 资源证据，隔离后重跑。
- **需求/文档不一致**：先收敛目标，再同步代码、测试、设计和用户文档。

### 最终交付模板

最终交付前必须准备固定摘要：

- **目标对齐**：逐条列出用户目标验证清单完成状态。
- **Review/Fix/Test 闭环**：列出第 1 轮、第 2 轮和追加轮次的发现、修复、复测结果。
- **验证矩阵**：单元测试、E2E、human_tests、`cargo test --workspace --all-features`、`scripts/ci/local-ci.sh`、远端 CI 的执行状态、命令、结果和未执行原因。
- **提交/MR/CI 状态**：列出分支、commit、MR/PR 链接、CI run id、CI 当前状态或全绿证据。
- **变更范围**：修改文件、未触碰的用户既有改动、临时文件清理状态。
- **残余风险**：未覆盖项、阻塞项或需用户决策事项。

### 文档/流程变更边界

仅修改文档、流程和测试说明时，Rust 单元测试、E2E、workspace all-features 和 local-ci 可以标记为不适用，但必须满足：

- 未修改 Rust、WebUI、脚本、配置、协议或运行行为。
- 已更新或创建对应 `human_tests/` 用例并逐条真实执行。
- 最终验证矩阵明确列出不适用原因。

### TodoWrite 要求

规划阶段必须把闭环拆成可执行 todo，至少包含：

- `任务启动检查：执行 git status --short --branch；如有既有修改或并行任务，创建/切换独立 worktree`
- `用户目标验证清单：必须实现 ...；必须不破坏 ...；必须真实验证 ...；必须交付 ...`
- `Review/Fix/Test 第 1 轮：目标复核 + 修改文件 review + 问题修复 + 相关测试运行`
- `Review/Fix/Test 第 2 轮：修复后复查 + 覆盖缺口检查 + 复跑相关测试`
- 如果继续循环，追加 `Review/Fix/Test 第 N 轮：...`
- `提交/MR/CI 看护：提交并推送任务分支 + 创建或更新 MR/PR + fail-fast 看护远端 CI`
- `最终交付自检：目标对齐 + 两轮闭环 + 验证矩阵 + 提交/MR/CI 状态 + 残余风险`

禁止把两轮闭环合并成一句“最终 review”或“跑测试”。

## 技术细节

### 关键文件

- `AGENTS.md`：主开发手册。相关章节：
  - `## 执行心法`
  - `## 执行模式与完成定义`（`### 任务模式判定`、`### 默认提交、MR 与 CI 看护策略`、`### 任务启动与工作区隔离`、`### 证据台账`、`### 完成定义`）
  - `## 开发需求标准流程`（`### 第一阶段：分析与规划` … `### 第五阶段：收尾`，`### 第四阶段：强制 Review/Fix/Test 闭环`）
  - `## 测试覆盖要求`（`### 真实场景测试（human_tests 驱动）` 强制执行）
  - `## 测试完备性检查清单`
- `design/agent-development-review-loop.md`（本文件）
- `human_tests/agent-development-review-loop.md`
- `human_tests/readme.md`
- `.agents/skills/github-actions-pat/`：CI 看护 skill。
- `.agents/skills/e2e-test/`：E2E 规范。
- `scripts/ci/local-ci.sh`：本地 CI wrapper；含 `--e2e-only shell`、`--e2e-only platform` 等模式。
- `rust-project-validate`：workspace 校验入口。

### 状态机契约

```text
START
  ├── mode = classify_task(user_request)
  ├── if working_tree_dirty || parallel_task_detected:
  │       create_or_switch_worktree()
  ├── verify_targets = build_user_goal_matrix()
  ├── plan = TodoWrite([...见上])
  ├── implement()
  ├── loop LOOP_ROUND in [1, 2, ...]:
  │       diff = git_status_and_diff()
  │       findings = code_review(diff, verify_targets)
  │       apply_fixes(findings)
  │       results = run_tests(relevant_scope(findings))
  │       if any_p0_p1_p2 or targets_incomplete:
  │           continue
  │       break_condition = LOOP_ROUND >= 2 && targets_all_verified
  │       if break_condition: break
  ├── if changed_files > 0 && !user_exempted:
  │       commit + push
  │       ensure_draft_mr()
  │       watch_ci_fail_fast()
  │       while ci_failed:
  │           fix -> push -> watch
  ├── deliver(final_summary_template)
END
```

### 与其它 design 文档的关系

- `design/agent-plan-lifecycle.md`：`update_plan` runtime 语义；本设计规定 Agent 使用它时不允许把两轮闭环的每一步长期塞进 plan（final 证据放交付摘要，不放进 plan）。
- `design/agent-loop-process-isolation.md`：worker 隔离；本设计执行 review 时若涉及 worker 相关文件（`agent_worker.rs` 等）必须按 `worker/pass-through/fail-closed` 三项额外检查。
- `design/agent-long-task-suspension.md`：Cooperative Long Task Loop；执行 review 时若涉及 `exec_command`、`RuntimeExecMonitor` 等文件必须按 `monitor/backoff/游标` 检查。

## CLI / Web / Admin API

本设计规范 Agent 内部行为，不新增 CLI、Web、Admin API。已有相关入口：

- CLI：`bifrost agent status` 显示当前 session 状态；`bifrost --help` 保留 hidden 子命令；`bifrost sync ...`、`bifrost rule ...` 等均遵循同一闭环。
- Web：`Settings → Agent` 页面；`Chat` 页面 SSE 流式；开发时 Playwright 覆盖点属于本闭环第 4 阶段。
- Admin API：`/_bifrost/api/agent/*`、`/_bifrost/api/im-gateway/*`；执行 review 时用真实 curl / websocat 复测。

## Sync 边界

- 本设计文档、`AGENTS.md`、`human_tests/*.md` 均属于仓库文档，通过 git 版本控制，不通过 `crates/bifrost-sync` 同步。
- 单次任务的 `TodoWrite` / review 记录属于 Agent 会话状态，不同步；两轮闭环的证据保留在最终交付回复与 MR/PR description 中。
- 远端 CI 状态通过 `.agents/skills/github-actions-pat/` 拉取，不参与业务 sync。
- Fork 或多仓库场景：本设计只适用于 `bifrost-proxy/bifrost` 主仓；其它 fork 需自行同步 `AGENTS.md` 内容。

## 实现切分

### Phase 1：文档骨架

- 更新 `AGENTS.md` 顶部持续改进引导语。
- 更新 `AGENTS.md` `## 执行模式与完成定义` 与 `## 开发需求标准流程` 章节，加入双轮闭环、任务模式判定、证据台账、完成定义、文档/流程变更边界。
- 更新本设计文档。

### Phase 2：human_tests 落地

- 更新 `human_tests/agent-development-review-loop.md`（当前 199 行），补齐 9 个 TC。
- 更新 `human_tests/readme.md` 索引；只维护索引条目，不维护全局汇总数字。

### Phase 3：默认提交 / MR / CI 看护

- 更新 `AGENTS.md` `### 默认提交、MR 与 CI 看护策略`：明确 fail-fast 看护、fix-push-watch 循环、最终交付状态汇报。
- 引用 `.agents/skills/github-actions-pat/` skill 使用方式。

### Phase 4：并行开发与 worktree

- 更新 `AGENTS.md` `### 任务启动与工作区隔离`：`git status --short --branch` 检查；并行开发默认独立 `git worktree`；豁免条件。

## 测试方案

### 单元测试

本次为流程文档变更，无 Rust 公共函数或核心逻辑修改，不新增单元测试。后续 Agent 处理代码开发任务时，仍必须按具体模块补充单元测试。

### E2E 测试

本次为流程文档变更，无可运行产品行为或 CLI/API 行为变化，不新增自动化 E2E 脚本。后续代码开发任务必须按 `.agents/skills/e2e-test/` 规范补充 E2E。

### 真实场景测试

更新 `human_tests/agent-development-review-loop.md`，覆盖：

- `TC-ADRL-01`：确认 `AGENTS.md` 的标准流程、规划要求、验证阶段、收尾门禁均包含两轮闭环。
- `TC-ADRL-02`：确认 `human_tests/readme.md` 索引包含该用例文档，并且测试总数同步更新。
- `TC-ADRL-03`：确认 `AGENTS.md` 和设计文档包含持续改进引导语。
- `TC-ADRL-04`：确认 `AGENTS.md` 包含任务模式判定、证据台账、完成定义和文档/流程变更测试边界。
- `TC-ADRL-05`：确认 `AGENTS.md` 包含用户目标验证清单、git diff/status 复核、测试失败归因和最终交付模板。
- `TC-ADRL-06`：确认开发流程阶段编号连续，无重复编号。
- `TC-ADRL-07`：确认 `AGENTS.md` 和设计文档包含任务启动时 `git status --short --branch` 检查，以及并行开发优先使用独立 worktree 的规则。
- `TC-ADRL-08`：确认 `AGENTS.md` 禁止 `human_tests/readme.md` 维护全局汇总数字。
- `TC-ADRL-09`：确认默认开发流程包含提交、推送、MR/PR 创建或更新、远端 CI fail-fast 看护和最终交付状态汇报。

### Coverage 与项目校验

- 执行 `rg` 验证 `AGENTS.md`、`design/agent-development-review-loop.md`、`human_tests/agent-development-review-loop.md`、`human_tests/readme.md` 中的闭环关键词和索引存在。
- 执行 `rg` 验证默认提交、MR/PR、远端 CI 看护、fix-push-watch 和最终交付状态汇报关键词存在。
- 执行 `git diff --check` 确认文档无尾随空白。
- 按 `human_tests/agent-development-review-loop.md` 逐条执行并记录结果。
- 本次未修改 Rust 代码，可不执行 `cargo test --workspace --all-features`；如后续任务包含代码变更，必须按 `AGENTS.md` 执行完整校验，包括 `cargo fmt`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`rust-project-validate`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`AGENTS.md` 与本设计文档在持续改进引导、任务模式判定、任务启动 `git status` + worktree、双轮闭环、用户目标验证清单、TodoWrite 要求、测试失败归因、最终交付模板、文档/流程变更边界、默认提交 + MR + CI 看护上完全一致。
- 执行 `git status --short --branch`、`git diff`、必要 `git diff --cached`。
- 复核 `human_tests/agent-development-review-loop.md` 9 个 TC 是否全部覆盖上述条目；`human_tests/readme.md` 索引是否存在。
- 关键点：`AGENTS.md` 与本设计不能出现互相冲突的表述；不能新增禁止在 readme 维护全局汇总数字的例外；`.agents/skills/github-actions-pat/` 引用路径正确。
- 复测：`rg` 关键词校验、`human_tests/agent-development-review-loop.md` 逐条 dry-run。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、未跟踪文件与 staged 状态。
- 重点检查：默认提交/CI 看护策略是否有回滚；task mode 判定与开发流程编号是否连续；文档/流程变更边界是否明确列出“Rust/E2E/workspace 不适用”的判定条件。
- 复跑 `rg` 关键词校验、`human_tests/agent-development-review-loop.md` 逐条执行。
- 若发现真实执行时某条 TC 无法自证，或 `AGENTS.md` 章节编号变化影响 TC 定位，追加第 3 轮修复。

## 风险与决策

- 双轮闭环显著增加单任务耗时。若任务规模极小（单文件 typo 修复），仍强制两轮可能收益递减；本设计选择“两轮为最低要求”，不因任务规模豁免——原因是历史事故多次显示“小改动的第二轮才发现真正影响”。
- fail-fast CI 看护依赖 `.agents/skills/github-actls-pat/`；若 token 权限不足或 GitHub API 限流，Agent 应带证据停止而非无限重试。
- 并行 worktree 会占用磁盘与 IDE 资源；`AGENTS.md` 允许在“用户明确要求继续”或“任务就是处理既有修改”时豁免，避免过度约束。
- 文档/流程变更边界允许 Rust 单元/E2E 不适用；但必须补 human_tests，否则容易出现“空跑文档变更”而没有真实验证。
- 本设计文档是元规范，本身也遵循两轮闭环：修改本文件仍必须执行 `human_tests/agent-development-review-loop.md` 全量校验，避免文档漂移。
