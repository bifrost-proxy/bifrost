# macOS Native App 方案

## 目标

新增 `apps/macos` 原生 macOS 客户端骨架，作为现有 Tauri 桌面端之外的 Native Preview。第一阶段只建立可验证的工程边界：SwiftUI/AppKit 做控制台体验，Rust `bifrost` CLI 继续作为 sidecar 负责代理、TLS、规则、脚本、存储和 Admin API。

## 非目标

- 不改造或删除 `desktop/` Tauri 桌面端。
- 不重写代理数据面。
- 不引入 Rust FFI、NetworkExtension 或透明代理。
- 不在本切片实现完整 Traffic/Rules 业务交互。

## 目录结构

```text
apps/macos/
  Package.swift
  Project.yml
  Sources/
    BifrostMac/
    BifrostMacCore/
  Tests/
    BifrostMacCoreTests/
```

SwiftPM 是第一阶段的强制可复现构建路径；`Project.yml` 仅作为后续 XcodeGen 工程入口。

## 实现逻辑

### BifrostMacCore

- `AdminAPIRequestFactory` 统一构造 `/_bifrost/api/` URL、`X-Client-Id`、`Authorization` 和 unsafe method 的 CSRF header。
- `BifrostClient` 作为 actor 包装 URLSession，第一阶段暴露 System、Traffic、Rules、Cert、Proxy 的 Data 级方法，避免在 schema 稳定前过早绑定大量 DTO。
- `SidecarConfiguration` 保持与现有 Tauri 端一致的默认值：默认端口 `9900`、sidecar 绑定 `0.0.0.0`、Admin 探测 `127.0.0.1`、数据目录默认 `~/.bifrost`。
- `SidecarManager` 先落启动命令计划、端口候选策略和最小 start/stop；开发默认加 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免污染开发机。

### BifrostMac

- 主界面使用 `NavigationSplitView`。
- Overview/Traffic/Rules 先提供可编译页面骨架。
- Traffic 表格从第一版使用 AppKit `NSTableView` bridge，给后续高吞吐流量列表留出性能边界。
- Rules 编辑器从第一版使用 AppKit `NSTextView` bridge，后续再补规则 DSL 高亮和诊断。

## 依赖项

- macOS 13+
- SwiftPM / Swift 5.9+
- Rust workspace 中现有 `bifrost-cli`
- 现有 Admin API：`/_bifrost/api/system/overview`、`/traffic`、`/rules`、`/cert`、`/proxy/address`、`/proxy/system`

## 测试方案

### 单元测试

- `AdminAPIRequestFactoryTests`
  - 验证 URL 前缀为 `/_bifrost/api/`。
  - 验证 query 参数构造。
  - 验证 unsafe request 附带 Client/Auth/CSRF/Content-Type headers。
  - 验证相对 path 被拒绝。
- `SidecarConfigurationTests`
  - 验证默认数据目录保持 `~/.bifrost`。
  - 验证 start command 包含 `--skip-cert-check`、`--no-system-proxy` 和安全环境变量。
  - 验证端口候选策略与 Tauri 的 preferred port + 递增窗口一致。

执行命令：

```bash
swift run --package-path apps/macos BifrostMacCoreChecks
```

### E2E 测试

本切片不改变 Rust daemon 运行行为，不新增代理规则语义。E2E 采用工程 smoke：

```bash
scripts/build-macos-native.sh --test
```

该命令构建 Rust sidecar、复制到 Native app 本地 sidecar 目录，并运行 SwiftPM build 与 `BifrostMacCoreChecks`。真实代理启动留到 SidecarManager 接入 UI 控制后补充。

### 真实场景测试

新增 `human_tests/macos-native-app.md`，覆盖：

- SwiftPM build/test。
- sidecar 准备脚本输出可执行文件。
- build 脚本 `--skip-sidecar --test` 可只验证 Swift 工程。
- sidecar 命令计划包含开发安全开关。
- `desktop/` Tauri 文件未被修改。
- Linux shell E2E CI 使用现有 shard 机制拆成 4 个 job，避免全量 shell 套件在单个 60 分钟 job 中超时，同时通过覆盖守卫确认用例不丢失。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：新增 native client，不改 Tauri，不做 FFI/NetworkExtension。
- 执行 `git status --short`、`git diff`。
- Review `apps/macos`、`scripts/build-macos-native.sh`、`scripts/prepare-macos-native-sidecar.sh`、`docs/macos-native.md`、`human_tests/macos-native-app.md`。
- 复跑 `swift run --package-path apps/macos BifrostMacCoreChecks` 和脚本 smoke。

### 第 2 轮

- 复查第 1 轮修复后的 diff 与 human_tests 索引。
- 确认脚本不启动系统代理，不拉起托盘，不触碰 Tauri 配置。
- 复跑受影响命令，并确认是否需要第 3 轮。

## 校验要求

- `swift run --package-path apps/macos BifrostMacCoreChecks`
- `scripts/build-macos-native.sh --skip-sidecar --test`
- `scripts/prepare-macos-native-sidecar.sh --skip-cargo-build` 在已有 binary 时可复用
- `cargo fmt --all -- --check`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo test --workspace --all-features`
- `cargo build --all-targets --all-features`
- `make coverage` 或记录覆盖率门禁对 Swift scaffold 的不适用/阻塞原因
- `bash scripts/ci/check-e2e-shell-ci-coverage.sh`
- YAML 检查确认 `.github/workflows/ci.yml` 的 Linux `e2e-shell` job 配置 `matrix.shard: [1, 2, 3, 4]`

## 文档更新要求

- 新增 `docs/macos-native.md`。
- 更新 `human_tests/readme.md` 索引。
- 当前不修改 README 顶层安装入口，避免把 Native Preview 误写成正式替代路径。
