# CA Install System Keychain 设计方案

## 背景

Bifrost 需要 MITM 目标 host 的 HTTPS 流量，前提是本机（或移动设备）信任 Bifrost 生成的自签根 CA。历史实现在 macOS 下只把 CA 安装到 login keychain，Safari、部分辅助进程（Cmd-line curl、Homebrew tap、桌面 App 内的 WebView）仍会提示证书不受信任。用户体验上表现为：Chrome / Firefox 一开始就绿锁，但同一台机器上其它软件仍然报错，看起来像是 Bifrost 装了一半。

同时，Bifrost 在 CLI 侧原本没有显式的 `ca install` 子命令，用户只能等 `bifrost start` 首次弹交互式提示才能安装；这在 CI、脚本化部署、远程调试、移动设备接入等场景都非常别扭。

本方案：

1. 新增 `bifrost ca install` 子命令，用于显式安装并信任 CA 证书；除桌面系统信任外，还支持 `--mobile` / `--ios` / `--configurator` / `--device <id>` / `--yes` 参数把 CA 推送到已连接的 Android 或 iOS 设备。
2. 在 macOS 下把本机 CA 安装流程固定为 `System.keychain`，并在安装前先清理同名证书在 login keychain 中的残留，避免仅写入登录钥匙串导致部分浏览器 / 辅助进程仍提示 HTTPS 不安全。
3. 把 `bifrost start` 交互路径和 `bifrost ca install` 收敛到同一个 `CertInstaller::install_and_trust()`，确保行为一致、可测试、可脚本化。

## 用户目标验证清单

### 必须实现

- `bifrost ca install` 子命令存在，`--help` 列出 `--mobile` / `--ios` / `--configurator` / `--device <id>` / `--yes` 等参数。
- 运行时先 `ensure_ca_exists`，不存在或失效时自动 `generate_root_ca` + `save_root_ca`。
- 桌面路径：`CertInstaller::install_and_trust()` 在 macOS 下写入 `/Library/Keychains/System.keychain` 并标记信任，不再回退到 login keychain。
- 安装前先 `purge_macos_named_certificates` 清掉 login keychain 中同名旧证书，避免 login keychain 残留。
- `check_status_macos` 只比对 System.keychain 指纹；即使 login keychain 仍残留同名证书，状态也会被判定为 `NotInstalled`，从而触发重新安装。
- `--mobile` 或显式 `--device` → `handle_mobile_ca_install`（默认走 Android ADB 安装）。
- `--ios` / `--configurator` → `handle_ios_ca_install`（生成 `.mobileconfig`，或经 Apple Configurator `cfgutil` 推送）。
- `bifrost start` 路径上的 `check_and_install_certificate`（携带 `CertificateCheckOptions { auto_yes, allow_prompt }`）和 `bifrost ca install` 最终都调用同一个 `CertInstaller::install_and_trust()`。
- 非交互环境未传 `--yes` 时以 `BlockNonInteractive` 报错，提示用户先跑 `bifrost ca install` 或加 `--yes` / `--skip-cert-check`。

### 必须不破坏

- Linux / Windows 现有 CA 安装路径保持原有实现语义（`install_and_trust` 仅在 macOS 分支变更策略）。
- `bifrost ca generate` / `bifrost ca export` / `bifrost ca info` 现有语义不变。
- Android ADB / iOS Apple Configurator 集成保留原有 `bifrost-device` 依赖与错误提示；未接入设备时输出可读引导。
- 桌面 GUI 路径 `install_and_trust_gui()` 仍走 `run_macos_security_add_trusted_cert_gui`，触发系统 UI 授权，不改成静默 sudo 安装。
- 已有 login keychain 中残留同名证书的用户，第一次运行新逻辑时按 “purge login + install system” 一次性完成，不留下奇怪的中间状态。

### 必须真实验证

- macOS 命令级：`bifrost ca install --help` / `bifrost ca install` / `bifrost ca install --mobile` / `bifrost ca install --ios --configurator` 在真实机器上真跑一遍。
- macOS 状态检查：手工把证书塞进 login keychain 后运行 `bifrost ca info`，判定为 `NotInstalled`；再跑 `bifrost ca install`，判定为 `Installed` 且 System.keychain 中出现 Bifrost CA。
- macOS 首次交互：干净数据目录下 `bifrost start` 命中未安装分支后走 `install_and_trust`，安装到 System.keychain。
- 非交互环境（如 CI）未传 `--yes` 时报 `BlockNonInteractive` 并给出正确提示。
- Android ADB / iOS Configurator 有真实设备连接时能推送成功；无设备时给出正确引导。

## 产品语义

### 一条命令、两个入口、一份实现

- `bifrost ca install`（显式，可脚本化，可远程调用）与 `bifrost start` 初次启动检查（交互式）背后走同一份 `install_and_trust`，保证行为可复现。
- `--mobile` / `--ios` 是显式子路由，仅覆盖“把证书推给设备”这一附加动作；桌面系统信任仍走同一份逻辑。
- `--yes` 用来跳过 CLI 端确认，服务端逻辑不变，仍走同一份安装 + 信任。

### macOS 只信任 System.keychain

macOS 上，Safari / Xcode Simulator / Homebrew / curl / Electron App 内部 WebView 只有在 System.keychain 中的证书才会被统一信任。login keychain 只能覆盖“当前登录用户的图形应用”，且不同工具对 login keychain 的读法不一致。因此本方案把 CA 安装固定到 System.keychain，并在安装前清空 login keychain 中同名残留：新证书装到 System.keychain，老证书从 login keychain 清掉，避免 “看起来装了两份” 的错觉。

### 状态检查以 System.keychain 为准

`bifrost ca info` / `bifrost ca install` 在 macOS 上一律读 System.keychain 的指纹与 Bifrost CA 匹配。这样即使有用户手工把老证书塞进 login keychain，Bifrost 也不会“误报已安装”，会主动重新安装。

## 技术细节

### 1. 新增 `ca install` 命令

- `crates/bifrost-cli/src/cli.rs`：在 `CaCommands` 中新增 `Install` 变体，携带 `mobile: bool` / `ios: bool` / `configurator: bool` / `device: Option<String>` / `yes: bool` 参数。
- `crates/bifrost-cli/src/commands/ca.rs::handle_ca_command`：
  - 先 `ensure_ca_exists` 校验本地 CA 文件；不存在或失效时自动 `generate_root_ca` + `save_root_ca`。
  - 分支调度：
    - `--ios` / `--configurator` → `handle_ios_ca_install`（生成 `.mobileconfig`，或经 Apple Configurator `cfgutil` 推送）。
    - `--mobile` 或显式 `--device` → `handle_mobile_ca_install`（默认走 Android ADB 安装）。
    - 否则走本机系统信任：`CertInstaller::install_and_trust()`。
  - 输出：安装成功时打印 “Installed Bifrost CA into System keychain”；非交互失败时打印 “Re-run with --yes to install and trust it automatically, run 'bifrost ca install' first, or pass --skip-cert-check”。
- `main.rs`：`Some(Commands::Ca { action }) => handle_ca_command(action)` 保持不变，只新增分支枚举值。

### 2. 调整 macOS 安装策略

`crates/bifrost-tls/src/install.rs`：

- `install_macos()` 改为：
  1. `purge_macos_named_certificates(&self.cert_name, &resolve_macos_login_keychain()?, false)` 清掉 login keychain 中同名旧证书。
  2. `install_macos_cert_to_system_keychain(&self.cert_path)` 写入 `/Library/Keychains/System.keychain` 并标记信任。
  3. 不再回退到登录钥匙串。
- `install_macos_gui()`（`install_and_trust_gui` 使用）走 `run_macos_security_add_trusted_cert_gui`，安装目标仍是 System.keychain。
- `check_status_macos` 只比对 System.keychain 指纹；即使 login keychain 仍残留同名证书，状态判定为 `NotInstalled`，触发重新安装。
- `install_and_trust()` 与 `install_and_trust_gui()` 是安装入口，其他子命令与 `bifrost start` 路径只调这两者。

### 3. `bifrost start` 路径

- `check_and_install_certificate` 携带 `CertificateCheckOptions { auto_yes, allow_prompt }`：
  - `auto_yes = true`（`--yes` / `--skip-cert-check` 相关）→ 直接调 `install_and_trust`。
  - `allow_prompt = true` → 弹交互提示，用户确认后调 `install_and_trust`。
  - 两者都为 false（非交互且未 `--yes`）→ 返回 `BlockNonInteractive`，提示先跑 `bifrost ca install` 或加 `--yes` / `--skip-cert-check`。

## CLI 交互

```
bifrost ca install                       # 桌面系统信任
bifrost ca install --yes                 # 非交互桌面系统信任（CI / 脚本）
bifrost ca install --mobile              # Android ADB 推送
bifrost ca install --mobile --device XX  # 指定 Android 设备
bifrost ca install --ios                 # 输出 .mobileconfig 引导
bifrost ca install --ios --configurator  # 通过 Apple Configurator cfgutil 推送
bifrost ca install --ios --configurator --device XX
```

错误提示（示例）：

- macOS 非交互未 `--yes`：`Bifrost CA is not trusted on this Mac. Re-run with --yes to install and trust it automatically, run 'bifrost ca install' first, or pass --skip-cert-check to bypass.`
- Android 未接设备：`No connected Android device was detected. Enable USB debugging, approve this computer on the phone, then re-run 'bifrost ca install --mobile'.`
- iOS 未接设备：`No connected iPhone/iPad was detected. Connect the device over USB, unlock it, tap Trust for this Mac if prompted, then re-run 'bifrost ca install --ios --configurator'.`
- macOS 状态检查：`bifrost ca info` 在 System.keychain 无匹配时输出 `Bifrost CA is not installed in System keychain. Run 'bifrost ca install' to install and trust the certificate.`

## Web / Admin

- Admin handler 的 `install_and_trust_gui()` 走 `CertInstaller::install_and_trust_gui()`；点击“信任证书”按钮时触发系统 UI 授权（macOS Security.framework），安装目标仍是 System.keychain。
- Admin API 状态查询与 CLI 共用 `CertInstaller::check_status()`，判定口径一致。
- Web 端在 macOS 上展示 “Installed to System keychain” 而不是 “Installed to login keychain”，避免用户误以为只是当前用户信任。

## Sync 边界

- CA 是本机安全资产，不参与任何跨设备 sync。
- `bifrost ca export` 允许用户手工导出 CA，供别的机器 / 设备信任；导入仍要用户在目标机器上显式操作。
- 远程调用（`bifrost remote` shell exec）不允许直接推 CA 到远端 System.keychain；远端需要单独跑 `bifrost ca install`。

## 实现切分

### Phase 1：CLI 与 dispatch

- `CaCommands::Install { mobile, ios, configurator, device, yes }` 结构新增。
- `handle_ca_command` 分支：`ensure_ca_exists` → iOS / mobile / desktop 三条路径。
- `handle_ios_ca_install` / `handle_mobile_ca_install` 补齐引导文案。
- `--help` 输出对齐产品语义。

### Phase 2：macOS 安装策略

- `install_macos()`：`purge_macos_named_certificates` → `install_macos_cert_to_system_keychain`。
- `install_macos_gui()`：`run_macos_security_add_trusted_cert_gui`，目标仍是 System.keychain。
- `check_status_macos` 仅比对 System.keychain 指纹。
- 单元测试覆盖“login 有残留 + system 无 → NotInstalled”“system 存在 → Installed”“同 name 但不同指纹 → NotInstalled”。

### Phase 3：`bifrost start` 收敛

- `check_and_install_certificate` 与 `bifrost ca install` 共用 `install_and_trust`。
- 非交互无 `--yes` → `BlockNonInteractive`，配合可测试错误文案。
- 交互路径与 `--yes` 路径均能命中 `install_and_trust`（用测试替身验证）。

### Phase 4：文档 & 移动端引导

- CLI help / README / docs 增加 `bifrost ca install` 章节。
- 移动端引导文案统一（Android ADB 授权、iOS Trust Profile）。
- `human_tests/ca-install-system-keychain.md` 更新真实执行步骤。

## 测试方案

### 单元测试

- `install::install_macos_only_writes_system_keychain`：mock `security` 命令验证只调 `-k /Library/Keychains/System.keychain`。
- `install::install_macos_purges_login_keychain_first`：安装前先执行 `purge_macos_named_certificates`。
- `install::check_status_macos_ignores_login_keychain`：login keychain 存在同名证书但 System.keychain 无匹配 → `NotInstalled`。
- `install::install_and_trust_gui_dispatches_to_gui_helper`：GUI 路径仍走 `run_macos_security_add_trusted_cert_gui`。
- `ca::ensure_ca_exists_regenerates_when_missing`：CA 文件不存在时自动生成并保存。
- `start::block_non_interactive_when_no_yes`：非交互且无 `--yes` 时返回 `BlockNonInteractive`，提示信息稳定可测试。
- `cli::ca_install_help_lists_mobile_ios_flags`：`bifrost ca install --help` 输出包含 `--mobile` / `--ios` / `--configurator` / `--device` / `--yes`。

### 命令级 / E2E 验证

- `bifrost ca install --help` 输出包含所有子选项。
- `cargo test -p bifrost-cli ca`。
- macOS 真实机器：
  - 干净环境下 `bifrost ca install` 后，`security find-certificate -c "Bifrost Root CA" /Library/Keychains/System.keychain` 命中。
  - login keychain 手动塞入旧证书 + `bifrost ca install` → login 里旧证书被清除，System 里存在新证书。
  - `bifrost ca info` 反映 System.keychain 状态。
  - `bifrost start` 未装证书时命中 `install_and_trust`，安装到 System.keychain。
- Android：手机连接 + `adb devices` 可见时，`bifrost ca install --mobile` 成功推送并提示用户信任。
- iOS：`bifrost ca install --ios --configurator` 在 `cfgutil` 环境下成功推送 profile；未接设备时按引导文案退出。

### 真实场景测试 human_tests

新增 / 更新 `human_tests/ca-install-system-keychain.md`：

- `TC-CA-01`：干净 macOS 环境显式 `bifrost ca install`，验证 System.keychain 装入、Safari 绿锁。
- `TC-CA-02`：login keychain 有残留 → 运行 `bifrost ca install` → login 清空 + System 安装。
- `TC-CA-03`：`bifrost start` 交互路径与 `bifrost ca install` 行为一致。
- `TC-CA-04`：非交互无 `--yes` → `BlockNonInteractive` 报错。
- `TC-CA-05`：Android ADB 推送真实设备。
- `TC-CA-06`：iOS Configurator `cfgutil` 真实设备推送。
- 所有用例服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-cli ca`
- `cargo test -p bifrost-tls install`
- `cargo build --all-targets --all-features`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定时不跑 `make coverage`；交付时说明本地豁免。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`bifrost ca install` 显式命令、macOS 系统信任固化到 System.keychain、login 残留清理、`bifrost start` 与 `ca install` 共用同一 `install_and_trust`。
- 复核 diff：`cli.rs` 新增枚举、`commands/ca.rs` 分支、`bifrost-tls/install.rs` macOS 分支、`check_status_macos`、`bifrost start` 错误分支。
- 重点 review：macOS 分支是否仍有 login keychain fallback；GUI 路径是否仍写 login；`ensure_ca_exists` 是否在所有子命令入口都被调用；非交互错误文案是否稳定。
- 复测：`cargo test -p bifrost-cli ca` / `cargo test -p bifrost-tls install`；macOS 真实机器验收；human_tests 真实执行。

### 第 2 轮

- 复查第 1 轮修复；再次 `git status --short` / `git diff`。
- 重点 review：ADB / cfgutil 无设备时错误文案；`bifrost ca info` 输出是否随之更新；README / docs 是否同步。
- 复测：失败路径重跑；`cargo test --workspace --all-features`；`rust-project-validate`。

## 风险与决策点

- **不再写 login keychain**：会不会有用户依赖 login keychain 里的证书？评估后：主流场景 System.keychain 覆盖所有工具，login-only 场景极少见；如果确实有工具只读 login keychain，用户可以在 System.keychain 装完后手工再拷一份到 login，不改默认路径。
- **`sudo` 权限**：`security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain` 需要 root。CLI 路径通过 `sudo` 提示用户输入密码；GUI 路径走 `run_macos_security_add_trusted_cert_gui`（触发系统 UI 授权，避免在无 TTY 时卡死）。
- **`--yes` 非交互**：为了脚本化，允许通过 `--yes` 跳过 CLI 确认；但 `sudo` 密码仍由 macOS 决定是否要求输入，未免密时脚本仍会阻塞——文档需明确提示 CI 环境要预配置 sudoers 或改用桌面 GUI 授权。
- **移动端**：Android ADB 与 iOS Apple Configurator 依赖外部工具与真实设备；未接设备时给出可执行引导文案，而不是失败静默。
- **迁移**：老用户升级到新版本后第一次跑 `bifrost ca install` 会一次性把 login keychain 里的老证书清掉；如果用户之前把该证书做过其它用途，需要重新导入——文档中显式提示。
