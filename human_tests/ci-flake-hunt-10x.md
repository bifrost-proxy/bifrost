# CI 稳定性 10 次压测

## 功能模块说明

验证从最新 `main` 切出的 PR 能触发完整 GitHub Actions CI，并通过 fail-fast 监控累计至少 10 次完整成功运行。若任一运行失败，必须先根据失败 job 日志归因，再修复不稳定因素并重新触发 CI。

## 前置条件

1. 当前目录为仓库根目录。
2. 当前分支为 `codex/ci-flake-hunt-10x`。
3. `GITHUB_TOKEN` 已在 shell 环境中配置。
4. PR 已创建到 `main`，且 PR diff 包含 `scripts/**` 下的触发文件以命中 `.github/workflows/ci.yml` 的 paths 过滤。

## 测试用例列表

### TC-CFH-01：PR 触发完整 CI 工作流

操作步骤：

1. 执行 `git status --short --branch`，确认当前分支为 `codex/ci-flake-hunt-10x`。
2. 执行 `gh pr checks 249 --watch=false` 或等效 GitHub Actions API 查询。
3. 确认 PR #249 已出现 CI checks。

预期结果：

1. PR #249 至少出现 `.github/workflows/ci.yml` 对应的 CI run。
2. CI run 包含 Rust Format Check、Rust Clippy Check、Rust Dependency Audit、Unit & Integration Tests、Coverage Gate、E2E 构建以及平台相关 E2E/打包 job。

### TC-CFH-02：fail-fast 监控失败并进入修复闭环

操作步骤：

1. 使用 `GH_REPO=bifrost-proxy/bifrost python3 .agents/skills/github-actions-pat/scripts/gh_ci.py branch codex/ci-flake-hunt-10x --any-status` 找到最新 run id。
2. 执行 `GH_REPO=bifrost-proxy/bifrost python3 .agents/skills/github-actions-pat/scripts/watch_jobs.py <run_id>`。
3. 若命令返回失败，读取输出中的失败 job、step、root-cause bucket 和日志片段。
4. 修复归因确认属于本仓库的不稳定因素后，提交、推送并重新触发 CI。

预期结果：

1. 任一失败 job 会被 fail-fast 监控捕获，不等待慢 job 全部结束。
2. 失败日志能够定位到具体 job、step 和关键错误片段。
3. 修复后重新触发的 CI run 继续纳入 10 次成功计数。

### TC-CFH-03：累计 10 次完整成功 CI

操作步骤：

1. 从 PR #249 的 CI run 或 run attempts 中记录每一次完整成功的 run id、attempt 和 head sha。
2. 每次成功后重新触发下一次 CI。
3. 重复直到记录到至少 10 次完整成功。

预期结果：

1. 累计成功次数不少于 10。
2. 每次成功记录都能在 GitHub Actions 中核验。
3. 最终交付包含 10 次成功 CI 的 run id、attempt、head sha 和状态。

## 清理步骤

- 本用例不启动本地 Bifrost 服务，不修改系统代理。
- 若后续发现并修复真实 CI 不稳定因素，按对应模块的清理步骤处理临时数据。

## 执行记录

| 日期 | 用例 | 操作 | 结果 |
| --- | --- | --- | --- |
| 2026-06-16 | TC-CFH-01 | 推送 `ff68f905` 后执行 `gh pr checks 249 --watch=false` 与 `gh_ci.py branch codex/ci-flake-hunt-10x --any-status` | 通过：PR #249 出现 CI run `27580795070`，checks 覆盖 Rust Format、Clippy、Dependency Audit、Unit & Integration、Coverage、E2E/Build 矩阵 |
| 2026-06-16 | TC-CFH-02 | 对 run `27580795070` attempt `2` 执行 `watch_jobs.py` fail-fast 看护 | 发现不稳定因素：Windows `E2E Rules (x86_64-pc-windows-msvc, shard 4/4)` 运行约 47 分钟后 hosted runner lost communication；GitHub job log blob 返回 404，原 PAT 脚本 traceback，需修复后重试 |
| 2026-06-16 | TC-CFH-02 | 修复 PAT 日志 404 容错后，对同一失败 run 执行 `watch_jobs.py 27580795070` | 通过：命令 exit 2，输出 `github actions runner/log unavailable` 结构化摘要，不再 traceback |
| 2026-06-16 | TC-CFH-03 | 对最新 head `f361f119` 的 run `27587458988` attempt `1` 执行 fail-fast watch | 通过：36/36 jobs 全绿，作为修复后成功计数 `1/10` |
| 2026-06-16 | TC-CFH-03 | 对 run `27587458988` attempt `2` 执行 fail-fast watch | 通过：36/36 jobs 全绿，作为修复后成功计数 `2/10` |
| 2026-06-16 | TC-CFH-02 | 对 run `27587458988` attempt `3` 执行 fail-fast watch | 发现不稳定因素：`Build E2E Binary` 在 `cargo build --release --bin bifrost` 阶段运行约 47 分钟后 hosted runner lost communication；GitHub job log blob 返回 404。修复要求：Linux release artifact build 降低 Cargo 并发并增加 heartbeat，后续重新推送并重新计数 |
