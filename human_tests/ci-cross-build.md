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
   rg -n 'Build \(cross\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml
   ```

**预期结果**：
- 能定位到 `Build (cross)` step。
- 该 step 的 `env` 中包含 `CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"`。
- release workflow 仍通过 `${{ matrix.target }}` 构建，不需要为每个 target 复制配置。

### TC-CCB-03: armv7 CI 失败回归验证

**操作步骤**：
1. 检查 `Cross.toml` 的 armv7 pre-build：
   ```bash
   rg -n 'armv7-unknown-linux-gnueabihf|https://archive\.ubuntu\.com/ubuntu|https://security\.ubuntu\.com/ubuntu|Acquire::Retries=5|clang libclang-dev' Cross.toml
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

### TC-CCB-05: PR path filter 最小触发回归

**操作步骤**：
1. 检查当前 PR 提交是否包含 `.github/workflows/ci.yml` 变更：
   ```bash
   git diff -- .github/workflows/ci.yml
   ```
2. 校验 workflow 与相关文档改动没有空白/格式错误：
   ```bash
   git diff --check -- .github/workflows/ci.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md
   ```
3. 推送包含 `.github/workflows/ci.yml` 变更的 commit 到当前 PR 分支。
4. 查询 PR #111 的最新 workflow 状态：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
   GH_REPO=bifrost-proxy/bifrost \
   python3 .agents/skills/github-actions-pat/scripts/gh_ci.py pr 111 --any-status
   ```

**预期结果**：
- 第 1 步输出显示 `.github/workflows/ci.yml` 已被纳入本次改动。
- 第 2 步无输出，说明本次最小变更没有引入格式问题。
- 推送后 PR #111 会创建新的 `CI` workflow run，而不是继续停留在旧 head 的 pending 状态。
- 如果新 run 出现失败，必须继续进入 fail-fast 分析与修复循环，不能停在“已触发”状态。

### TC-CCB-06: merge main 后 Windows rules timeout 配置不退化

**操作步骤**：
1. 检查 Windows rules job 的 suite timeout 配置来源：
   ```bash
   rg -n 'BIFROST_E2E_SUITE_TIMEOUT|suite_timeout: "4800"|suite_timeout: "7200"' .github/workflows/ci.yml
   ```
2. 确认 workflow 中不再残留固定 `4800` 的 env 配置：
   ```bash
   rg -n '^\s+BIFROST_E2E_SUITE_TIMEOUT: "4800"' .github/workflows/ci.yml && exit 1 || true
   ```
3. 校验相关文档与 workflow merge 结果没有格式错误：
   ```bash
   git diff --check -- .github/workflows/ci.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md human_tests/rules-e2e-fixtures.md
   ```

**预期结果**：
- `rg` 输出显示 `.github/workflows/ci.yml` 中只存在一条 `BIFROST_E2E_SUITE_TIMEOUT`，其值为 `${{ matrix.suite_timeout }}`。
- Windows rules matrix 仍包含 `suite_timeout: "4800"`（x86_64）与 `suite_timeout: "7200"`（Windows ARM）。
- 第 2 步无输出且不报错，说明 merge main 后没有把固定 `4800` env 配置带回 workflow。
- `git diff --check` 无输出，说明 merge 结果与关联文档格式正常。

### TC-CCB-07: Windows ARM Rules E2E 下线回归

**操作步骤**：
1. 检查 `e2e-windows-rules` matrix 中是否仍包含 Windows ARM 条目：
   ```bash
   sed -n '880,950p' .github/workflows/ci.yml | rg -n 'windows-11-arm|aarch64-pc-windows-msvc|windows-latest|x86_64-pc-windows-msvc'
   ```
2. 检查 workflow 其他 job 是否仍保留 Windows ARM 平台覆盖：
   ```bash
   rg -n 'windows-11-arm|aarch64-pc-windows-msvc' .github/workflows/ci.yml
   ```
3. 校验本次 workflow 与文档改动没有格式问题：
   ```bash
   git diff --check -- .github/workflows/ci.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md
   ```
4. 推送后查询 PR #111 的最新 CI：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
   GH_REPO=bifrost-proxy/bifrost \
   python3 .agents/skills/github-actions-pat/scripts/gh_ci.py pr 111 --any-status
   ```

**预期结果**：
- 第 1 步输出只包含 `windows-latest` / `x86_64-pc-windows-msvc` 的 Rules E2E matrix 条目，不再包含 `windows-11-arm` / `aarch64-pc-windows-msvc`。
- 第 2 步仍能检索到 Windows ARM 条目，说明只下线了 Rules E2E，没有移除 Windows ARM 的其他 CI 覆盖。
- 第 3 步无输出，说明 workflow 与文档格式正常。
- 第 4 步对应的新 CI run 中，不再出现 `E2E Rules (aarch64-pc-windows-msvc)` job；如出现其他失败，继续按 fail-fast 流程处理。

## 清理步骤

- 无本地服务与临时数据目录需要清理。

## 执行记录

- 2026-05-04：通过。执行 `rg -n 'cross build -p bifrost-cli --release --target' .github/workflows/ci.yml`，输出 4 个 PR CI cross build step；执行 `rg -n 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/ci.yml`，输出 4 个 buildkit 禁用配置；执行 `rg -n 'Build \(cross\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml`，确认 release matrix cross build step 同步配置。
- 2026-05-04：通过。执行 `rg -n 'armv7-unknown-linux-gnueabihf|https://archive\.ubuntu\.com/ubuntu|https://security\.ubuntu\.com/ubuntu|Acquire::Retries=5|clang libclang-dev' Cross.toml`，确认 armv7 pre-build 会切换到 HTTPS Ubuntu 源，并用 `Acquire::Retries=5` 安装 `clang libclang-dev`。云端 TC-CCB-04 由推送后的 GitHub Actions `CI` run 验证。
- 2026-05-06：TC-CCB-05 本地检查通过。执行 `git diff -- .github/workflows/ci.yml`，确认当前改动包含 `.github/workflows/ci.yml`；执行 `git diff --check -- .github/workflows/ci.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md` 无输出；执行 `rg -n 'cross build -p bifrost-cli --release --target' .github/workflows/ci.yml`、`rg -n 'CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/ci.yml`、`rg -n 'Build \(cross\)|cross build -p bifrost-cli --release --target|CROSS_CONTAINER_ENGINE_NO_BUILDKIT: "1"' .github/workflows/release.yml` 与 `rg -n 'armv7-unknown-linux-gnueabihf|https://archive\.ubuntu\.com/ubuntu|https://security\.ubuntu\.com/ubuntu|Acquire::Retries=5|clang libclang-dev' Cross.toml`，确认原 4 个用例预期仍成立。远端“新 CI run 已触发”部分将在本次 commit push 后立即继续验证。
- 2026-05-06：TC-CCB-06 本地检查通过。执行 `rg -n 'BIFROST_E2E_SUITE_TIMEOUT|suite_timeout: "4800"|suite_timeout: "7200"' .github/workflows/ci.yml`，确认 matrix 保留 x86_64=4800、Windows ARM=7200，且 env 仅使用 `${{ matrix.suite_timeout }}`；执行 `rg -n '^\s+BIFROST_E2E_SUITE_TIMEOUT: "4800"' .github/workflows/ci.yml && exit 1 || true` 无输出，确认 merge main 后未回退为固定 4800；执行 `git diff --check -- .github/workflows/ci.yml design/ci-cross-build.md human_tests/ci-cross-build.md human_tests/readme.md human_tests/rules-e2e-fixtures.md` 无输出。
- 2026-05-07：TC-CCB-07 本地静态检查待执行；第 4 步远端 CI 验证将在本次改动 push 后立即执行并记录结果。
