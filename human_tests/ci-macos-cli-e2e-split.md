# CI macOS CLI/E2E 构建拆分

## 功能模块说明

验证 macOS CI 中 E2E 用例只等待 aarch64 CLI 构建，不再等待 x86_64 CLI 或 desktop/Tauri bundle 构建完成。desktop bundle 仍复用对应 CLI artifact 作为 sidecar，并独立完成桌面构建验证。

## 前置条件

- 工作目录：项目根目录 `<REPO_ROOT>`
- 当前 bifrost 正式代理保持运行在 `127.0.0.1:9900`
- 如需访问网络，使用：
  - `HTTP_PROXY=http://127.0.0.1:9900`
  - `HTTPS_PROXY=http://127.0.0.1:9900`
- 本用例只做 CI workflow 静态验证，不启动 Bifrost 测试实例，不修改系统代理。

## 测试用例

### TC-CMCE-01: macOS rules E2E 只依赖 aarch64 CLI 构建

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 检查 `e2e-macos-rules.needs`。
3. 检查该 job 下载的 artifact 名称。

**预期结果**：
- `e2e-macos-rules.needs` 等于 `build-cli-macos-aarch64`。
- 下载 artifact 名称为 `bifrost-release-aarch64-apple-darwin`。
- 不依赖 `build-cli-macos-x86_64`、`bundle-desktop-macos` 或任何 desktop matrix job。

### TC-CMCE-02: macOS shell E2E 分片只依赖 aarch64 CLI 构建

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 检查 `e2e-macos-shell.needs`。
3. 检查该 job 下载的 artifact 名称。

**预期结果**：
- `e2e-macos-shell.needs` 等于 `build-cli-macos-aarch64`。
- 下载 artifact 名称为 `bifrost-release-aarch64-apple-darwin`。
- 三个 shard 都不需要等待 x86_64 CLI 或 desktop bundle。

### TC-CMCE-03: macOS CLI 构建与 desktop bundle 职责分离

**操作步骤**：
1. 解析 `.github/workflows/ci.yml`。
2. 检查 `build-cli-macos-aarch64` 和 `build-cli-macos-x86_64` 的构建命令。
3. 检查 `bundle-desktop-macos.needs` 与下载 artifact 配置。

**预期结果**：
- 两个 CLI job 都执行 `cargo build -p bifrost-cli --release --target <target>`。
- 两个 CLI job 都设置 `SKIP_FRONTEND_BUILD=1`。
- `bundle-desktop-macos.needs` 包含两个 CLI job。
- `bundle-desktop-macos` 仍下载 `bifrost-release-${{ matrix.target }}`，供 `prepare-tauri-sidecar.mjs` 使用。

### TC-CMCE-04: workflow YAML 可解析

**操作步骤**：
1. 使用 YAML 解析器读取 `.github/workflows/ci.yml`。
2. 确认 `jobs` 下存在 `build-cli-macos-aarch64`、`build-cli-macos-x86_64`、`e2e-macos-rules`、`e2e-macos-shell`、`bundle-desktop-macos`。

**预期结果**：
- YAML 解析无异常。
- 所有关键 job 均存在。

### TC-CMCE-05: macOS desktop bundle 使用真实 Cargo/Rustc

**操作步骤**：
1. 检查 `bundle-desktop-macos` 的 Rust tool path 校验步骤：
   ```bash
   rg -n 'Verify Rust tool paths|rustup which cargo|rustup which rustc' .github/workflows/ci.yml
   ```
2. 检查 `Build macOS desktop bundle` 步骤：
   ```bash
   rg -n 'Build macOS desktop bundle|export CARGO="\\$\\(rustup which cargo\\)"|export RUSTC="\\$\\(rustup which rustc\\)"|pnpm exec tauri build' .github/workflows/ci.yml
   ```
3. 推送当前分支并观察 GitHub Actions `CI` workflow。

**预期结果**：
- macOS desktop bundle job 在执行 Tauri 前输出真实 `cargo` 与 `rustc` 版本。
- `tauri build` 在同一个 shell step 中通过 `CARGO` 和 `RUSTC` 环境变量使用 `rustup which` 解析到的真实工具链二进制。
- 不再出现 `cargo metadata` 实际调用到 `rustup-init` 并报 `unexpected argument 'metadata'` 的失败。
- `Bundle macOS (aarch64-apple-darwin)` 和 `Bundle macOS (x86_64-apple-darwin)` 最终进入 success。

### TC-CMCE-06: macOS desktop bundle Rust toolchain 安装具备重试

**操作步骤**：
1. 检查 PR CI desktop bundle 的 Rust toolchain 安装步骤：
   ```bash
   rg -n 'Install Rust toolchain with retry|command -v rustup|https://sh\\.rustup\\.rs|rustup toolchain install stable --target "\\$\\{target\\}" --profile minimal --no-self-update|attempt \\* 20|Failed to install stable Rust toolchain' .github/workflows/ci.yml
   ```
2. 检查 release workflow macOS desktop bundle 的 Rust toolchain 安装步骤：
   ```bash
   rg -n 'Install Rust toolchain with retry|runner\\.os == '\\''macOS'\\''|command -v rustup|https://sh\\.rustup\\.rs|rustup toolchain install stable --target "\\$\\{target\\}" --profile minimal --no-self-update|attempt \\* 20|Failed to install stable Rust toolchain' .github/workflows/release.yml
   ```
3. 推送当前分支并观察 GitHub Actions `CI` workflow。

**预期结果**：
- PR CI 的 `bundle-desktop-macos` 在安装 stable toolchain 时最多重试 3 次。
- 如果 macOS runner 缺少 `rustup`，会先通过 `https://sh.rustup.rs` bootstrap，并把 cargo bin 目录加入后续步骤的 `GITHUB_PATH`。
- 每次重试使用同一个 `${{ matrix.target }}`，包含 `--profile minimal` 和 `--no-self-update`。
- 第 1、2 次失败后按 `attempt * 20` 秒递增等待，第 3 次仍失败才让 job 失败。
- release workflow 的 macOS desktop bundle 使用同一重试策略；非 macOS bundle 保持 `dtolnay/rust-toolchain@stable`。
- 如果 `static.rust-lang.org` 出现短暂 DNS 抖动，macOS desktop bundle 有机会自动恢复；远端 `Bundle macOS (x86_64-apple-darwin)` 最终进入 success。

## 清理步骤

- 无清理需求；本测试不创建临时服务实例、不写入数据目录、不修改系统代理。

## 执行记录

- 2026-05-15：通过。执行 `rg -n 'Verify Rust tool paths|rustup which cargo|rustup which rustc' .github/workflows/ci.yml` 与 `rg -n 'Build macOS desktop bundle|export CARGO="\\$\\(rustup which cargo\\)"|export RUSTC="\\$\\(rustup which rustc\\)"|pnpm exec tauri build' .github/workflows/ci.yml`，确认 macOS desktop bundle 在 Tauri 构建前校验真实工具链，并在同一 shell step 中导出 `CARGO/RUSTC`；远端 TC-CMCE-05 由后续 GitHub Actions `CI` run 验证。
- 2026-05-28：通过。执行 `rg -n 'Install Rust toolchain with retry|command -v rustup|https://sh\\.rustup\\.rs|rustup toolchain install stable --target "\\$\\{target\\}" --profile minimal --no-self-update|attempt \\* 20|Failed to install stable Rust toolchain' .github/workflows/ci.yml`，确认 PR CI macOS desktop bundle toolchain 安装保留 rustup bootstrap 且具备 3 次重试；执行 `rg -n 'Install Rust toolchain with retry|runner\\.os == '\''macOS'\''|command -v rustup|https://sh\\.rustup\\.rs|rustup toolchain install stable --target "\\$\\{target\\}" --profile minimal --no-self-update|attempt \\* 20|Failed to install stable Rust toolchain' .github/workflows/release.yml`，确认 release macOS desktop bundle 同步具备重试且非 macOS 仍走 `dtolnay/rust-toolchain@stable`。远端 TC-CMCE-06 由推送后的 GitHub Actions `CI` run 验证。
