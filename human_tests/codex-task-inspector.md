# Codex Task Inspector 真实场景测试用例

## 功能模块说明

`codex-task-inspector` 是一个仓库级只读 skill，用于统一检查 Codex 异步任务进展。

**关键点**：

- 当用户给的是 **rollout/session id（如 `019...`）** 并问“任务进展”，应优先检查 **Codex 默认数据目录 `~/.codex`**（session/rollout jsonl 是权威来源）。
- 只有当用户明确指向“仓库内任务跟踪文件”时，才检查 `.codex-tasks/`。

本用例覆盖“误用 `.codex-tasks` 导致找不到任务”的回归，确保后续不再重复这个错误。

## 前置条件

```bash
cd <REPO_ROOT>
ls .codex-tasks
```

要求仓库内已经存在 `.codex-tasks/` 目录及至少一组：

- `*.pid`
- `*-last.md`
- `*.log`

## 测试用例列表

### TC-CTI-00：rollout/session id 必须走 ~/.codex（回归用例）

- **操作步骤**：
  1. 选取一个已知 rollout id（例如 `019df414-235e-74e3-be4b-84f883e0ea17`，或你本机存在的任意 id）
  2. 在 `~/.codex/sessions` 下定位 `rollout-*-<id>.jsonl`
  3. 读取 jsonl 最后一条 `task_complete` 的 `last_agent_message`（或等效的最终状态）
- **预期结果**：
  - 能在 `~/.codex` 中定位到该 id 的日志文件
  - 能明确给出任务是否已完成、最终结论、以及最后更新时间
  - 不会因为仓库里 `.codex-tasks/` 不存在对应文件而误判“没有进展/还在跑”


### TC-CTI-01：本地 pid 状态与任务文件解耦

- **操作步骤**：
  1. 执行 skill 中推荐的 pid 检查脚本，遍历 `.codex-tasks/*.pid`
  2. 记录每个 pid 的 `ps` 结果
  3. 同时确认 `.codex-tasks/` 中仍然存在多个任务文件
- **预期结果**：
  - 即使 `.codex-tasks/` 中仍有大量文件，只要 `ps` 无返回进程行，就判定对应任务 `NOT_RUNNING`
  - 输出结论明确区分“本地进程未运行”和“任务文件仍存在”

### TC-CTI-02：识别最近任务产物结论

- **操作步骤**：
  1. 列出最近的 `.codex-tasks/*-last.md`
  2. 读取最近 3~5 个文件的首个非空结论行
  3. 人工确认这些文件对应的任务结论能被摘要表达
- **预期结果**：
  - 能提取出最近任务的已完成/阻塞结论
  - 输出中将这些内容归类为“任务产物摘要”，而不是本地运行态

### TC-CTI-03：识别 CI poll 中的运行中与失败项

- **操作步骤**：
  1. 读取 `.codex-tasks/skill-creator-ci-poll.log` 尾部 40 行左右
  2. 找出 `completed/failure`、`in_progress/pending` 的 job
  3. 记录至少一个失败项和至少一个仍在运行项（若存在）
- **预期结果**：
  - 能正确识别失败 job 与运行中 job
  - 输出中明确标记这是 CI 状态，不与本地 pid 状态混淆

### TC-CTI-04：最终汇总结构符合固定四段

- **操作步骤**：
  1. 基于以上三步结果，组织一份最终汇总
  2. 检查是否包含四个部分：本地 Codex 进程、任务产物摘要、CI 状态、下一步建议
- **预期结果**：
  - 汇总至少包含四段固定结构
  - 下一步建议只在“继续查失败原因 / 整理状态表 / 展开具体报告”这类范围内

## 清理步骤

本测试为只读检查，无需清理。

## 执行记录

- TC-CTI-00：通过（2026-05-05）
  - 实际结果：在 `~/.codex/sessions/**/rollout-*-019df414-235e-74e3-be4b-84f883e0ea17.jsonl` 成功定位到 1 个匹配文件；读取到 `task_complete` 且首行结论为“CI 已处理到全绿。”
- TC-CTI-01：通过（2026-05-05）
  - 实际结果：`.codex-tasks/*.pid` 中列出的 pid 均为 `NOT_RUNNING`；即使 `.codex-tasks/` 目录仍有大量文件，也不会误判任务仍在运行。
- TC-CTI-02：通过（2026-05-05）
  - 实际结果：从最近 5 个 `*-last.md` 提取到了明确的首行结论（含“已完成”“push 失败（github.com 解析失败）”等），可稳定用于摘要。
- TC-CTI-03：通过（2026-05-05）
  - 实际结果：从 `skill-creator-ci-poll.log` 尾部识别到 `completed/failure`（E2E Shell Linux shard 3/3）与多个 `in_progress/pending` job；并将其归类为 CI 状态，不与本地 pid 混淆。
- TC-CTI-04：通过（2026-05-05）
  - 实际结果：最终汇总按四段输出（本地进程/产物摘要/CI 状态/下一步建议）组织，且建议不越界。

