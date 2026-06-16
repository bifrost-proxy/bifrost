# CI Flake Hunt 10x

## 功能模块说明

PR #249 用于从最新 `main` 创建独立分支并连续压测 GitHub Actions CI，目标是至少获得 10 次完整成功运行。任何失败都必须先归因，再修复本仓库可控的不稳定因素后重新计数。

## 实现逻辑

- 初始空提交不会触发 CI，因为 `.github/workflows/ci.yml` 的 `pull_request` paths 过滤不覆盖空 diff；因此分支保留 `scripts/ci/ci-flake-hunt-10x.md` 作为 CI 触发文件。
- run `27580795070` attempt `2` 暴露 Windows rules hosted runner lost communication，修复点记录在 `design/ci-windows-e2e-runner.md`。
- run `27587458988` attempt `3` 暴露 Linux `Build E2E Binary` 在 `cargo build --release --bin bifrost` 阶段 hosted runner lost communication，GitHub 只保留 annotation，job log signed blob 返回 404。
- Linux release build 是多个 E2E job 的前置 artifact 构建，不能简单跳过。该 job 改为显式 `CARGO_BUILD_JOBS=2` 并使用 `cargo build --jobs "$CARGO_BUILD_JOBS"`，降低 GitHub hosted runner 在 release 编译阶段的 CPU/内存压力。
- 构建 step 增加 60 秒 heartbeat。heartbeat 不改变产物，但能让长时间编译保留进度输出；如果后续仍失败，GitHub 更可能保留可读日志而不是只有 blob 404。
- CI 看护脚本必须把每一次 rerun 的失败视为失败，不能在 watcher 非零退出后继续计入成功。

## 依赖项

- `.github/workflows/ci.yml`
- `.trae/skills/github-actions-pat/scripts/watch_jobs.py`
- `human_tests/ci-flake-hunt-10x.md`
- `scripts/ci/ci-flake-hunt-10x.md`

## 测试方案

### 单元测试

本次改动为 GitHub Actions workflow、设计文档和 human_tests，不修改 Rust 业务逻辑；Rust 单元测试不适用。通过 YAML 解析和 workflow 静态断言验证配置。

### E2E 测试

- 静态解析 `.github/workflows/ci.yml`，确认 `build-e2e` 的 `Build release binary` step 使用 Bash、多行脚本、`CARGO_BUILD_JOBS=2`、heartbeat 和 `cargo build --release --bin bifrost --jobs "$CARGO_BUILD_JOBS"`。
- 推送后使用 GitHub Actions 真实 CI run 验证 `Build E2E Binary` 不再在 release 编译阶段失联。
- 连续 rerun PR #249 的 CI，累计最新 head 至少 10 次完整成功；任一失败都进入 fix-push-watch 闭环。

### 真实场景测试

- 更新 `human_tests/ci-flake-hunt-10x.md`，记录 `Build E2E Binary` runner lost communication 回归。
- 执行 workflow 静态检查，确认 Linux E2E artifact release build 已限制并发并输出 heartbeat。
- 推送后继续执行 PR #249 真实 GitHub Actions CI 看护。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核 attempt `3` 的失败 job annotation、workflow diff、设计文档和 human_tests；运行 YAML 解析与 build-e2e 静态断言。
- 第 2 轮：复查修复后的最新 diff、CI watcher 失败计数策略和远端 CI 结果；若仍有失败，继续追加修复轮次。

## 校验要求

- `ruby -ryaml -e 'YAML.load_file(".github/workflows/ci.yml")'`
- 静态断言 `build-e2e` 的 build step 包含 `CARGO_BUILD_JOBS=2`、heartbeat 和 `--jobs "$CARGO_BUILD_JOBS"`。
- `git diff --check`
- GitHub Actions 最新 `CI` run fail-fast watch。

## 文档更新要求

- 更新 `human_tests/ci-flake-hunt-10x.md`
- 如本次后续修复触及其他 CI 模块，同步更新对应 `design/` 文档和 human_tests。
