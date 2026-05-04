# CI Cross Build

## 功能模块说明

验证 GitHub Actions 中所有 Linux `cross build` 路径都显式禁用 Docker buildkit，避免 `Linux Build (armv7)` 在 runner 缺少 buildx/buildkit 能力时失败。

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
1. 推送包含本修复的 commit 到 `feat/agent`。
2. 使用 GitHub Actions PAT skill 查询新 head 的 `CI` workflow。
3. fail-fast 监控 `CI` run。

**预期结果**：
- `Linux Build (armv7)` 不再输出 `Suggestion: is buildx available for the container engine?` 后失败。
- `Linux Build (armv7)` job 进入 success。
- 如果后续出现其他 job 失败，继续按日志归因修复，但不能把 armv7 buildkit 失败当作已通过。

## 清理步骤

- 无本地服务与临时数据目录需要清理。

## 执行记录

- 2026-05-04：通过。执行 `rg -n 'cross build -p bifrost-cli --release --target' .github/workflows/ci.yml`，输出 4 个 PR CI cross build step；执行 `rg -n 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/ci.yml`，输出 4 个 buildkit 禁用配置；执行 `rg -n 'Build \\(cross\\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml`，确认 release matrix cross build step 同步配置。云端 TC-CCB-03 由推送后的 GitHub Actions `CI` run 验证。
