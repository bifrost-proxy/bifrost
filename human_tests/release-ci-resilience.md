# Release CI Resilience

## 功能模块说明

验证 Release workflow 对发布过程中的短暂基础设施失败具备自动恢复能力：artifact blob storage 忙时重试上传，crates.io DNS 或索引瞬断时在构建前重试 `cargo fetch`。同时验证 PR CI 中 macOS ARM shell E2E 对真实长尾测试有足够 job 预算，不因所有脚本通过后的收尾阶段超时而误报失败。

## 前置条件

- 在仓库根目录执行。
- 不启动 Bifrost 服务，不修改系统代理。
- 需要 `rg` 与 `python3` 可用。
- 需要通过 `github-actions-pat` 能读取 GitHub Actions run `28038797261` 的失败日志。

## 测试用例列表

### TC-RCR-01: Artifact 上传失败自动重试

**操作步骤**：
1. 检查 release workflow 的 CLI 与 Desktop artifact 上传均使用本地 retry action：
   ```bash
   rg -n 'Upload artifact|\\.github/actions/upload-artifact-with-retry|desktop-\\$\\{\\{ matrix.target \\}\\}|cli-\\$\\{\\{ matrix.target \\}\\}' .github/workflows/release.yml
   ```
2. 检查本地 retry action 的三次上传与 overwrite 配置：
   ```bash
   rg -n 'Upload artifact \\(attempt [123]\\)|overwrite: true|Fail when all upload attempts fail|retry-delay-seconds' .github/actions/upload-artifact-with-retry/action.yml
   ```

**预期结果**：
- 第 1 步能看到 CLI 与 Desktop 两个 `Upload artifact` step 都使用 `./.github/actions/upload-artifact-with-retry`。
- 第 2 步能看到 attempt 1、attempt 2、attempt 3 三个上传步骤。
- 三个上传步骤都设置 `overwrite: true`。
- 三次都失败时 action 会显式 `exit 1`，不会假装上传成功。

### TC-RCR-02: CLI build 前预取 Rust 依赖并重试

**操作步骤**：
1. 检查 release workflow 的 CLI build job 包含 root workspace cargo fetch 重试：
   ```bash
   rg -n 'Fetch Rust dependencies with retry|cargo fetch --target "\\$\\{target\\}"|Failed to fetch Rust dependencies' .github/workflows/release.yml
   ```

**预期结果**：
- CLI build job 中的 `Fetch Rust dependencies with retry` 位于 cargo cache 之后、`Build (native)` / `Build (cross)` 之前。
- 命令使用 `cargo fetch --target "${target}"`。
- 脚本最多尝试 3 次，并在第 3 次失败后返回非 0。

### TC-RCR-03: Desktop/Tauri build 前预取 root 与 desktop Rust 依赖并重试

**操作步骤**：
1. 检查 release workflow 的 Desktop build job 同时预取 root workspace 和 Tauri manifest 依赖：
   ```bash
   rg -n 'cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target "\\$\\{target\\}"|Build desktop frontend|Build bundled CLI backend|Build macOS desktop app bundle' .github/workflows/release.yml
   ```
2. 在本机验证新增 fetch 命令可执行：
   ```bash
   cargo fetch --target x86_64-apple-darwin && cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-apple-darwin
   ```

**预期结果**：
- Desktop build job 的 `Fetch Rust dependencies with retry` 位于 cargo cache 之后、`Build desktop frontend` 之前。
- 脚本同时执行 `cargo fetch --target "${target}"` 和 `cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target "${target}"`。
- macOS Tauri bundle 构建前已经完成带重试的 crates.io 依赖预取。
- 本机 `cargo fetch` 命令返回 0，不引入编译或服务启动副作用。

### TC-RCR-04: Release 失败日志与修复点匹配

**操作步骤**：
1. 通过 GitHub Actions PAT skill 查询失败 run：
   ```bash
   zsh -ic 'source ~/.zshrc >/dev/null 2>&1 || true; GH_REPO=bifrost-proxy/bifrost NO_PROXY=api.github.com,github.com,*.blob.core.windows.net HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= python3 .agents/skills/github-actions-pat/scripts/gh_ci.py run 28038797261'
   ```
2. 对比输出中的失败 job 与本次修复点。

**预期结果**：
- `Build Desktop (aarch64-pc-windows-msvc)` 的关键错误包含 artifact blob storage `The server is busy`，对应 TC-RCR-01 的上传重试。
- `Build Desktop (x86_64-apple-darwin)` 的关键错误包含 `Could not resolve host: index.crates.io`，对应 TC-RCR-03 的 Tauri 构建前 cargo fetch 重试。

### TC-RCR-05: macOS ARM shell E2E job 预算覆盖长尾 shard

**操作步骤**：
1. 检查 PR CI 的 macOS ARM shell E2E job timeout：
   ```bash
   sed -n '706,714p' .github/workflows/ci.yml
   ```
2. 检查失败 run `28043895224` 的失败 job 与 artifact 日志：
   ```bash
   rg -n 'Test Summary|PASS|passed|failed|All tests passed|所有测试通过' /tmp/bifrost-ci-28043895224/logs/.e2e-reports
   ```

**预期结果**：
- `E2E Shell (aarch64-apple-darwin, shard ${{ matrix.shard }}/2)` 的 `timeout-minutes` 为 `90`。
- 失败 run 的 shard 2 artifact 日志中未发现真实断言失败，失败归因为 60 分钟 job 预算不足。
- 增加 job timeout 不改变 shard 数量、测试命令或覆盖范围。

## 清理步骤

- 无本地服务、临时数据目录或代理配置需要清理。

## 执行记录

- 2026-06-24：通过。执行 `rg -n 'Upload artifact|\\.github/actions/upload-artifact-with-retry|desktop-\\$\\{\\{ matrix.target \\}\\}|cli-\\$\\{\\{ matrix.target \\}\\}' .github/workflows/release.yml`，确认 CLI 与 Desktop artifact 上传均使用本地 retry action；执行 `rg -n 'Upload artifact \\(attempt [123]\\)|overwrite: true|Fail when all upload attempts fail|retry-delay-seconds' .github/actions/upload-artifact-with-retry/action.yml`，确认三次上传、`overwrite: true` 和最终失败退出逻辑存在。
- 2026-06-24：通过。执行 `rg -n 'Fetch Rust dependencies with retry|cargo fetch --target "\\$\\{target\\}"|Failed to fetch Rust dependencies' .github/workflows/release.yml`，确认 CLI build job 与 Desktop build job 都有三次 `cargo fetch --target "${target}"` 重试。
- 2026-06-24：通过。执行 `rg -n 'cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target "\\$\\{target\\}"|Build desktop frontend|Build bundled CLI backend|Build macOS desktop app bundle' .github/workflows/release.yml`，确认 Desktop/Tauri manifest fetch 位于 desktop frontend、bundled CLI 和 macOS Tauri bundle 构建之前；执行 `cargo fetch --target x86_64-apple-darwin && cargo fetch --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-apple-darwin` 返回 0，并同步了当前 main 滞后的 root 与 desktop Cargo lockfile package version。
- 2026-06-24：通过。执行 `ruby -e 'require "yaml"; ARGV.each { |p| YAML.safe_load(File.read(p), permitted_classes: [], aliases: true); puts "#{p}: yaml ok" }' .github/workflows/release.yml .github/actions/upload-artifact-with-retry/action.yml`，确认 workflow 与本地 action YAML 可解析；执行 GitHub Actions PAT 查询 run `28038797261`，确认 `Build Desktop (aarch64-pc-windows-msvc)` 失败点为 artifact blob storage `The server is busy`，`Build Desktop (x86_64-apple-darwin)` 失败点为 `Could not resolve host: index.crates.io`，分别对应本次上传重试与 cargo fetch 重试修复。
- 2026-06-24：通过。执行 `sed -n '706,714p' .github/workflows/ci.yml`，确认 macOS ARM shell E2E job timeout 为 90 分钟；下载 CI run `28043895224` 的 `e2e-shell-logs-aarch64-apple-darwin-shard-2.zip` artifact 后执行 `rg -n 'FAIL|FAILED|failed|timeout|Timeout|timed out|ERROR|Error|✗|❌|panic|killed|Terminated' /tmp/bifrost-ci-28043895224/logs/.e2e-reports` 与 completion-marker 检查，确认日志中没有真实断言失败，job 失败来自 60 分钟长尾超时。
