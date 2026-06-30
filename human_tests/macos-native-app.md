# macOS Native App 真实场景测试

## 功能模块说明

验证 `apps/macos` SwiftUI/AppKit 原生客户端骨架可以在本机可复现构建，并且第一阶段只作为控制面接入现有 Rust sidecar 与 Admin API，不破坏现有 `desktop/` Tauri 桌面端。

## 前置条件

- macOS 13 或更高版本。
- 本机可执行 `swift`、`xcodebuild`、`cargo`。
- 仓库根目录为当前工作目录。
- 如需执行 sidecar 准备脚本，`target/debug/bifrost` 已存在，或允许脚本先执行 `cargo build -p bifrost-cli --bin bifrost`。

## 测试用例

### TC-MNA-01：SwiftPM core contract 检查通过

**操作步骤：**
1. 在仓库根目录执行：
   ```bash
   swift run --package-path apps/macos BifrostMacCoreChecks
   ```

**预期结果：**
- SwiftPM 成功解析 `apps/macos/Package.swift`。
- `BifrostMacCoreChecks` 输出 `BifrostMacCoreChecks passed`。
- 测试覆盖 Admin API URL/header 构造、默认数据目录、sidecar 启动参数和端口候选策略。

### TC-MNA-02：Native build 脚本可只验证 Swift 工程

**操作步骤：**
1. 在仓库根目录执行：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```

**预期结果：**
- 命令成功完成。
- 不要求构建 Rust sidecar。
- Swift app 与 core target 均可构建，单元测试通过。

### TC-MNA-03：sidecar 准备脚本输出可执行文件

**操作步骤：**
1. 如本机已有 `target/debug/bifrost`，执行：
   ```bash
   scripts/prepare-macos-native-sidecar.sh --skip-cargo-build
   ```
2. 如不存在，执行：
   ```bash
   scripts/prepare-macos-native-sidecar.sh
   ```
3. 检查脚本输出路径是否存在且可执行。

**预期结果：**
- 输出路径为 `apps/macos/.build/sidecar/bin/bifrost`。
- 文件存在且具备可执行权限。
- 脚本不启动 `bifrost start`，不修改系统代理。

### TC-MNA-04：sidecar 启动计划包含开发安全开关

**操作步骤：**
1. 执行：
   ```bash
   swift run --package-path apps/macos BifrostMacCoreChecks
   ```

**预期结果：**
- core contract 检查通过。
- start plan 参数包含 `--skip-cert-check` 与 `--no-system-proxy`。
- 环境变量包含 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和独立 `BIFROST_DATA_DIR`。

### TC-MNA-05：现有 Tauri 桌面端未被本切片改动

**操作步骤：**
1. 执行：
   ```bash
   git diff -- desktop package.json scripts/prepare-tauri-sidecar.mjs
   ```

**预期结果：**
- `desktop/`、Tauri sidecar 准备脚本和现有 desktop npm scripts 没有非预期 diff。
- Native Preview 与 Tauri 继续并行存在。

## 清理步骤

```bash
rm -rf apps/macos/.build/sidecar
```
