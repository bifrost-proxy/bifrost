# CA Install System Keychain

## 功能模块详细描述

- 新增 `bifrost ca install` 子命令，用于显式安装并信任 CA 证书；除桌面系统信任外，还支持 `--mobile` / `--ios` / `--configurator` / `--device <id>` / `--yes` 参数将 CA 推送到已连接的 Android 或 iOS 设备。
- macOS 下将本机 CA 安装流程固定为 `System.keychain`，并在安装前先清理同名证书在 login keychain 中的残留，避免仅写入登录钥匙串导致部分浏览器/辅助进程仍提示 HTTPS 不安全。

## 实现逻辑

### 1. 新增 `ca install` 命令

- 在 CLI 的 `CaCommands` 中新增 `Install` 变体（`crates/bifrost-cli/src/cli.rs`），携带 `mobile` / `ios` / `configurator` / `device` / `yes` 等参数。
- 运行时先 `ensure_ca_exists` 校验本地 CA 文件；不存在或失效时自动 `generate_root_ca` + `save_root_ca`。
- 分支调度（`crates/bifrost-cli/src/commands/ca.rs::handle_ca_command`）：
  - `--ios` / `--configurator` → `handle_ios_ca_install`（生成 .mobileconfig，或经 Apple Configurator cfgutil 推送）。
  - `--mobile` 或显式 `--device` → `handle_mobile_ca_install`（默认走 Android ADB 安装）。
  - 否则走本机系统信任：`CertInstaller::install_and_trust()`。

### 2. 调整 macOS 安装策略

- `CertInstaller::install_macos()`（`crates/bifrost-tls/src/install.rs`）改为：
  - 先 `purge_macos_named_certificates` 清掉 login keychain 中同名旧证书。
  - 仅通过 `install_macos_cert_to_system_keychain` 写入 `/Library/Keychains/System.keychain` 并标记信任，不再回退到登录钥匙串。
- 同名 GUI 变体 `install_macos_gui()` 走 `run_macos_security_add_trusted_cert_gui`，由 admin handler 的 `install_and_trust_gui()` 使用，安装目标仍是 System keychain。
- `check_status_macos` 只比对 System.keychain 指纹；即使 login keychain 仍残留同名证书，状态也会被判定为 `NotInstalled`，从而触发重新安装。
- `start` 路径上的 `check_and_install_certificate`（携带 `CertificateCheckOptions { auto_yes, allow_prompt }`）和 `bifrost ca install` 最终都调用同一个 `CertInstaller::install_and_trust()`；非交互且未传 `--yes` 时会以 `BlockNonInteractive` 报错，提示用户先跑 `bifrost ca install` 或加 `--yes` / `--skip-cert-check`。

## 依赖项

- 复用 `bifrost-tls::CertInstaller`（`install_and_trust` / `install_and_trust_gui` / `check_status` / `purge_macos_named_certificates`）。
- 复用 `bifrost-tls` 的 CA 生成与保存逻辑（`ensure_valid_ca` / `generate_root_ca` / `save_root_ca`）。
- 移动端推送复用 `bifrost-device`（`install_android_ca`、`generate_ios_mobileconfig`、`install_ios_profile_with_configurator` 等）。

## 测试方案（含 e2e）

- 命令级验证：
  - `bifrost ca install --help`
  - `cargo test -p bifrost-cli`
- 行为验证：
  - macOS 下确认安装仅写入 `System.keychain`
  - 仅存在 `login keychain` 证书时，状态检查仍判定为未安装
  - `start` 交互路径复用同一安装逻辑

## 校验要求（含 rust-project-validate）

- 先执行本次修改相关测试
- 再执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - 按修改范围执行 `cargo test`
  - `cargo build --all-targets --all-features`

## 文档更新要求

- CLI 帮助文案需新增 `ca install`
- 如 README 后续维护 CLI 示例，可补充该命令
