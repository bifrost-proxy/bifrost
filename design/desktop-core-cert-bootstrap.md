# 桌面端内嵌 Core 证书预检与自动安装

## 现状结论

当前桌面端的真实行为是：

- 内嵌 `bifrost` core 仍以 `--skip-cert-check` 非交互方式启动
- backend ready 后，桌面壳层再异步执行 CA 预检与 GUI 安装
- `BIFROST_DATA_DIR` 会优先决定证书与日志的落盘目录
- macOS 的桌面安装目标是 `System.keychain`

也就是说，CLI 的终端交互证书流程没有直接搬进桌面端；桌面端走的是“先启动 core，再由壳层补做 GUI 证书安装”的路径。

## 当前实现

### 1. 桌面壳层负责延迟证书预检

- `desktop/src-tauri/src/main.rs` 在 backend bootstrap 成功后，会启动独立线程延迟约 2 秒执行证书预检。
- 数据目录由 `bifrost_storage::data_dir()` 解析，优先级当前为：
  - 进程内 `set_data_dir()` 覆盖值（CLI / 测试启动时注入）
  - 进程环境变量 `BIFROST_DATA_DIR`
  - `~/.bifrost`（无 home 时回退到 `./.bifrost`）
- 证书预检流程：
  - 确保 `certs/` 目录存在
  - 若 `ca.crt` / `ca.key` 缺失或无效，则生成新的根证书
  - 使用 `CertInstaller::check_status()` 检查系统是否已安装并信任
- 若未安装或未信任，则调用适合桌面 GUI 的安装入口
- 实现上会把预检结果写入 `logs/desktop-bootstrap.log`，包括：
  - 是否新生成了 `ca.crt` / `ca.key`
  - 当前系统信任状态
  - 是否触发 GUI 安装 / 是否被用户取消

### 2. Core 继续保持非交互启动

- 桌面壳层在 core ready 之后才执行预检，内嵌 core 仍然带 `--skip-cert-check` 启动。
- 这样可以避免：
  - CLI 在后台再次进入 `dialoguer` 交互
  - Tauri 壳层与 CLI 双重提示
  - 启动时因为后台进程无终端而卡死
  - 启动阶段因系统授权弹窗阻塞而白屏

### 3. GUI 场景下的证书安装策略

- macOS：
  - 桌面端走 `CertInstaller::install_and_trust_gui()`。
  - 实际安装目标是 `System.keychain`。
  - login keychain 不再作为成功状态兜底。
- Windows：
  - 继续复用当前的 UAC 提权安装逻辑。
- Linux：
  - 继续复用已有安装能力。
  - 若当前环境无法提供图形化提权，则记录失败并继续启动桌面端，不把整个桌面应用阻塞在证书安装上。

### 4. 安装失败与取消授权的处理

- 如果用户取消系统授权，或安装命令失败：
  - 桌面端仍继续启动内嵌 core
  - 不中断主窗口展示
  - 后续 HTTPS 拦截是否可用，仍由证书实际信任状态决定
- 这样可以保证桌面端始终可打开，用户仍可在设置页查看证书、手工安装或重试。

## 依赖项

- `desktop/src-tauri/src/main.rs`（`ensure_desktop_cert_ready` / `prepare_desktop_certificates` / `schedule_desktop_cert_ready`）
- `desktop/src-tauri/Cargo.toml`
- `crates/bifrost-tls/src/install.rs`（`CertInstaller` / `CertStatus` / `install_and_trust_gui`）
- `crates/bifrost-storage/src/data_dir.rs`（`data_dir()` 解析逻辑）
- `README.md`

## 测试方案（含 e2e）

1. 从默认目录启动桌面端，确认内嵌 core 正常拉起。
2. 从自定义 `BIFROST_DATA_DIR` 启动桌面端，确认 CA 与日志都写入该目录。
3. 在 macOS 下删除临时数据目录后首次启动桌面端，确认 backend ready 后才出现系统授权弹窗。
4. 若授权通过，确认 `System.keychain` 中出现当前 `Bifrost CA` 且状态变为 `InstalledAndTrusted`。
5. 若授权取消，确认桌面端仍可继续进入主窗口。
6. 检查 `logs/desktop-bootstrap.log`，确认包含证书预检与 GUI 安装日志。

## 校验要求（含 rust-project-validate）

- 先执行本次改动相关的桌面 / E2E 验证
- 再执行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`（按修改范围执行）
  - `cargo build --all-targets --all-features`

## 文档更新要求

- README 需要与当前行为保持一致：
  - 桌面端会在 backend 启动后异步执行 CA 预检
  - macOS / Windows 首次安装时可能弹出系统授权框
  - 桌面端支持通过 `BIFROST_DATA_DIR` 覆盖数据目录
