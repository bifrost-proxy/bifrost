# CI Cross Build

## 功能模块说明

验证 GitHub Actions 中所有 Linux `cross build` 路径都显式禁用 Docker buildkit，并验证 armv7 cross 容器 pre-build 使用 HTTPS Ubuntu 源与 apt retry，避免 `Linux Build (armv7)` 在 runner 缺少 buildx/buildkit 或 HTTP apt mirror 不稳定时失败。

## 前置条件

- 在仓库根目录执行。
- 不启动 Bifrost 服务，不使用 9900 端口。
- 需要 `rg` 可用。

## 测试用例列表

### TC-CCB-01: PR CI cross build 禁用 buildkit

**操作步骤**：
1. 检查 PR CI workflow 中所有 cross build 命令：
   ```bash
   rg -n 'cross build -p bifrost-cli --release --target' .github/workflows/ci.yml
   ```
2. 检查 PR CI workflow 中 buildkit 禁用变量：
   ```bash
   rg -n 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/ci.yml
   ```

**预期结果**：
- 第 1 步输出 4 个 `cross build` step。
- 第 2 步输出 4 个 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"` 配置。
- 4 个 cross target 分别为 `aarch64-unknown-linux-gnu`、`armv7-unknown-linux-gnueabihf`、`x86_64-unknown-linux-musl`、`aarch64-unknown-linux-musl`。

### TC-CCB-02: Release cross build 禁用 buildkit

**操作步骤**：
1. 检查 release workflow 的 matrix cross build step：
   ```bash
   rg -n 'Build \\(cross\\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml
   ```

**预期结果**：
- 能定位到 `Build (cross)` step。
- 该 step 的 `env` 中包含 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- release workflow 仍通过 `${{ matrix.target }}` 构建，不需要为每个 target 复制配置。

### TC-CCB-03: armv7 CI 失败回归验证

**操作步骤**：
1. 检查 `Cross.toml` 的 armv7 pre-build：
   ```bash
   rg -n 'armv7-unknown-linux-gnueabihf|https://archive\\.ubuntu\\.com/ubuntu|https://security\\.ubuntu\\.com/ubuntu|Acquire::Retries=5|clang libclang-dev' Cross.toml
   ```

**预期结果**：
- 能定位到 armv7 target 配置。
- pre-build 会把 Ubuntu archive/security 源从 HTTP 改为 HTTPS。
- `apt-get update` 和 `apt-get install` 使用 `Acquire::Retries=5`。
- 仍安装 `clang libclang-dev`，不改变 armv7 构建依赖。

### TC-CCB-04: armv7 CI 失败远端回归验证

**操作步骤**：
1. 推送包含本修复的 commit 到 `feat/agent`。
2. 使用 GitHub Actions PAT skill 查询新 head 的 `CI` workflow。
3. fail-fast 监控 `CI` run。

**预期结果**：
- `Linux Build (armv7)` 不再输出 `Suggestion: is buildx available for the container engine?` 后失败。
- `Linux Build (armv7)` 不再因为 `archive.archive.ubuntu.com:80` 连接失败而在 `clang/libclang-dev` pre-build 阶段失败。
- `Linux Build (armv7)` job 进入 success。
- 如果后续出现其他 job 失败，继续按日志归因修复，但不能把 armv7 buildkit 失败当作已通过。

### TC-CCB-05: aarch64-musl GHCR 拉取超时重试

**操作步骤**：
1. 检查 PR CI workflow 中 `Linux Build (aarch64-musl)` 的 cross build step：
   ```bash
   rg -n 'Linux Build \\(aarch64-musl\\)|cross build attempt|aarch64-unknown-linux-musl|sleep \\$\\(\\(attempt \\* 10\\)\\)' .github/workflows/ci.yml
   ```
2. 推送包含本修复的 commit 到当前 PR 分支。
3. 使用 GitHub Actions PAT skill 查询新 head 的 `CI` workflow，并 fail-fast 监控 run。

**预期结果**：
- `Linux Build (aarch64-musl)` 的 `cross build` 命令最多重试 3 次。
- 第 1、2 次失败后分别等待递增时间再重试。
- 如果 GHCR/cross 容器镜像拉取出现短暂超时，job 有机会自动恢复；第 3 次仍失败时保留失败结论。
- 远端 `Linux Build (aarch64-musl)` job 进入 success；如失败，必须按日志继续归因，不能把网络超时误判为代码失败。

## 清理步骤

- 无本地服务与临时数据目录需要清理。

## 执行记录

- 2026-05-04：通过。执行 `rg -n 'cross build -p bifrost-cli --release --target' .github/workflows/ci.yml`，输出 4 个 PR CI cross build step；执行 `rg -n 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/ci.yml`，输出 4 个 buildkit 禁用配置；执行 `rg -n 'Build \\(cross\\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml`，确认 release matrix cross build step 同步配置。
- 2026-05-04：通过。执行 `rg -n 'armv7-unknown-linux-gnueabihf|https://archive\\.ubuntu\\.com/ubuntu|https://security\\.ubuntu\\.com/ubuntu|Acquire::Retries=5|clang libclang-dev' Cross.toml`，确认 armv7 pre-build 会切换到 HTTPS Ubuntu 源，并用 `Acquire::Retries=5` 安装 `clang libclang-dev`。云端 TC-CCB-04 由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-15：通过。执行 `rg -n 'Linux Build \\(aarch64-musl\\)|cross build attempt|aarch64-unknown-linux-musl|sleep \\$\\(\\(attempt \\* 10\\)\\)' .github/workflows/ci.yml`，确认 `Linux Build (aarch64-musl)` 的 `cross build` step 有 3 次重试与递增等待；远端 TC-CCB-05 由后续 GitHub Actions `CI` run 验证。
