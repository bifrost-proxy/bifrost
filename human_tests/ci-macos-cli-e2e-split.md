# CI macOS CLI/E2E 构建拆分

## 功能模块说明

验证 macOS CI 中 E2E 用例只等待 aarch64 CLI 构建，不再等待 x86_64 CLI 或 desktop/Tauri bundle 构建完成。desktop bundle 仍复用对应 CLI artifact 作为 sidecar，并独立完成桌面构建验证。

## 前置条件

- 工作目录：项目根目录 `/Users/eden/work/github/bifrost`
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

## 清理步骤

- 无清理需求；本测试不创建临时服务实例、不写入数据目录、不修改系统代理。
