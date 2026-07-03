# 桌面端内嵌 Core 证书预检与自动安装

## 背景

Bifrost 桌面端把 CLI `bifrost` 作为 sidecar 内嵌启动。CLI 的原生证书路径需要 `dialoguer` 交互确认根 CA 是否安装 / 信任，而桌面端 sidecar 是非交互进程，一旦触发终端 prompt 会直接卡死；同时，macOS 首次 `security add-trusted-cert` 会弹系统授权框，如果在启动最早期就弹，用户看到的是白屏 Tauri 窗口，体验差且易被用户误认为闪退。

因此桌面端采用“先启动 core、再由壳层补做 GUI 证书安装”的分层策略：

- 内嵌 core 通过 `--skip-cert-check` 启动，绕过 CLI 交互路径。
- Tauri 壳层在 backend ready 后，延迟约 2 秒 spawn 独立线程执行证书预检、生成/复用 CA、必要时调用 GUI 提权安装。
- 全过程写入 `<data_dir>/logs/desktop-bootstrap.log`，方便离线排障。
- 授权失败或用户取消，桌面端不阻断，主窗口照常展示，HTTPS 拦截能力则回退到“未信任”状态。

本方案冻结该分层策略、明确日志格式与失败降级，并强调 `BIFROST_DATA_DIR` 优先级。

## 用户目标验证清单

### 必须实现

- 桌面端启动时，内嵌 core 使用 `--skip-cert-check` 参数，不进入终端交互路径。
- Backend ready（`is_backend_ready(port)` 返回 true）后，Tauri 壳层调用 `schedule_desktop_cert_ready(data_dir)` 延迟 2 秒 spawn 独立 OS 线程执行 `ensure_desktop_cert_ready`。
- `ensure_desktop_cert_ready` 覆盖：`certs/` 目录创建、`ca.crt`/`ca.key` 有效性检查、必要时 `generate_root_ca()` + `save_root_ca()`、`CertInstaller::check_status()` 状态判定、非 `InstalledAndTrusted` 时调用 `install_and_trust_gui()`、最后再次 `check_status()` 归档结果。
- 数据目录优先级严格为：`bifrost_storage::set_data_dir()` 进程内覆盖 → `BIFROST_DATA_DIR` 环境变量 → `~/.bifrost` → `./.bifrost`（无 home 时兜底）。
- macOS 上 GUI 安装目标为 `System.keychain`，login keychain 不再作为“成功”状态兜底。
- 全部预检步骤、结果、错误、用户取消都追加到 `<data_dir>/logs/desktop-bootstrap.log`。

### 必须不破坏

- CLI `bifrost start` 单独运行时的证书交互路径不变，`dialoguer` prompt 继续存在。
- Windows UAC 提权安装逻辑不变；Linux 图形提权失败时降级为“继续启动 + 记录失败”而非阻塞。
- 桌面壳层已有的 `bootstrap_desktop_backend()`、`ensure_backend_running()`、`try_start_native_handoff()` 等主流程时序不变。
- `BIFROST_DATA_DIR` 也会决定 `logs/`、`certs/`、`traffic/`、`bifrost.toml` 落盘位置，本方案不覆盖这条既有行为。

### 必须真实验证

- macOS 首次启动（临时 `BIFROST_DATA_DIR`）：backend ready 后弹系统授权框；授权通过后 `security find-certificate -c "Bifrost CA" /Library/Keychains/System.keychain` 找到证书；`CertStatus` 为 `InstalledAndTrusted`。
- macOS 用户点击取消：桌面主窗口仍能进入，代理仍启动，`desktop-bootstrap.log` 记录 `cancelled by user; continuing startup without trusted CA`。
- 自定义 `BIFROST_DATA_DIR` 启动：`ca.crt`、`ca.key`、`desktop-bootstrap.log` 全部写入该目录，`~/.bifrost` 无污染。
- 手工删除 `certs/ca.crt` 后重启：新 CA 被生成并再次触发 GUI 安装。

## 产品语义

### 桌面端 = “后弹权限”，不阻断启动

Tauri 主窗口 4~5 秒内必须显示。证书弹窗放在延迟线程，是为了：

- 避免 macOS `SFAuthorizationView` 与 Tauri window init 争夺主线程焦点，出现白屏。
- 让用户先看到应用界面再回答“是否允许安装证书”，语义更明确。
- 用户拒绝 → 只是 HTTPS 拦截能力受限，其他能力（traffic、非 TLS 请求、CLI 分发）仍可用。

### 证书状态是运行时数据

`CertStatus` 有 4 值：`NotInstalled` / `InstalledNotTrusted` / `InstalledAndTrusted` / `UserCancelled`（后者通过错误消息判定）。桌面端不在 UI 上把状态“藏起来”—— 设置页会读同一 `CertInstaller::check_status()` 展示。

### CA 生成一次、后续复用

`ensure_valid_ca(ca_cert_path, ca_key_path)` 判定“文件存在 + PEM 可解析 + 未过期”。有效则复用；无效则 `generate_root_ca()` 生成新 CA 并覆盖写入。用户手动删除 `certs/ca.crt` 后下一次启动会自动重建。

## 技术细节

### 桌面壳层入口

`desktop/src-tauri/src/main.rs`:

- `bootstrap_desktop_backend(app_handle)`：启动 sidecar + `wait_for_backend` + 触发 `schedule_desktop_cert_ready`。
- `schedule_desktop_cert_ready(data_dir)`：`thread::spawn` + `sleep(2s)` + `ensure_desktop_cert_ready`。
- `ensure_desktop_cert_ready(data_dir)`：调用 `prepare_desktop_certificates`，把结果分 4 类日志写入 bootstrap log。
- `prepare_desktop_certificates(data_dir)`：
  1. `fs::create_dir_all(certs/)`
  2. `ensure_valid_ca()` → 无效则 `generate_root_ca()` + `save_root_ca()`。
  3. `CertInstaller::new(&ca_cert_path).check_status()`
  4. `!= InstalledAndTrusted` 时 `install_and_trust_gui()`
  5. 再次 `check_status()` 返回最终状态。

### Sidecar 命令行

```rust
Command::new(binary_path)
    .args(["start", "--host", BACKEND_BIND_HOST, "--port", &port, "--skip-cert-check"])
    .env("BIFROST_DATA_DIR", data_dir)
    .stdout(...).stderr(...)
    .spawn()
```

`--skip-cert-check` 是核心：跳过 CLI 交互式 CA 检查。

### 数据目录解析

`bifrost_storage::data_dir()` 已按优先级实现，桌面端不再自行拼接路径。所有子目录（`certs/`、`logs/`、`traffic/`）均从此派生。

### GUI 安装实现

- macOS：`install_and_trust_gui()` 调用 `security execute-with-privileges + add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain`，触发系统授权框。
- Windows：`certutil -addstore -f "ROOT" ca.crt` 走 UAC。
- Linux：调用系统 GUI 提权工具（`pkexec` / `gksudo`），失败即降级日志。

### 日志格式

`desktop-bootstrap.log` 每行前缀时间戳，内容示例：

```
[2026-06-17T11:03:35Z] starting deferred desktop certificate preflight after startup
[2026-06-17T11:03:37Z] generated desktop CA certificate at /Users/x/.bifrost/certs/ca.crt
[2026-06-17T11:03:37Z] desktop CA status is NotInstalled; attempting GUI install/trust
[2026-06-17T11:03:44Z] desktop certificate preflight complete; CA was installed and trusted
```

## CLI 与 Admin API

本方案不新增 CLI 参数或 Admin API 端点。已有能力：

- CLI `bifrost cert status`：读取 `<data_dir>/certs/ca.crt` 并调用 `CertInstaller::check_status()`。
- CLI `bifrost cert install`：桌面外的手动安装路径（与 GUI 路径共用底层 installer）。
- Admin API `GET /_bifrost/api/cert/status`：Web UI 设置页读取当前信任状态。

Web UI 设置页 → Certificate 面板会展示 `desktop-bootstrap.log` 最新一条 preflight 结果，用户可点击“重新安装”触发 `install_and_trust_gui()`。

## 实现切分

### Phase 1：分层启动骨架

- 桌面端强制 sidecar `--skip-cert-check`。
- 引入 `schedule_desktop_cert_ready` 延迟线程。
- 打通 `bootstrap_desktop_backend → schedule_desktop_cert_ready`。

### Phase 2：CA 生命周期

- `ensure_valid_ca()` + `generate_root_ca()` + `save_root_ca()` 归入 storage。
- `CertInstaller::check_status()` + `install_and_trust_gui()` 与桌面壳层解耦。
- 用户取消 / 权限失败 → 明确 `UserCancelled` 分支日志。

### Phase 3：桌面 UI 反馈

- 设置页 Certificate 面板读实时状态。
- 首次安装完成后主动 push notification “Bifrost CA 已安装到 System keychain”。
- 失败时 UI 提供“重试安装”按钮。

### Phase 4：文档与人工用例

- README 说明桌面端 backend ready 后异步预检。
- `human_tests/desktop-core-cert-bootstrap.md` 覆盖首次 / 取消 / 删除 CA / 自定义 data_dir 场景。

## 测试方案

### 单元测试

- `bifrost-tls`：`CertInstaller::check_status`、`CertStatus::is_installed/is_trusted` 已有；不新增。
- 桌面壳层 `prepare_desktop_certificates` 目前无法直接 unit test（会真的调 `security`），改用 integration test with fake installer trait（后续优化）。
- `ensure_valid_ca` 与 `generate_root_ca` 有 `bifrost-tls::tests::test_ca_generation_roundtrip` 覆盖。

### E2E 测试

- 新增 `e2e-tests/tests/test_desktop_cert_bootstrap.sh`（可选，需 macOS runner）：
  - 临时 `BIFROST_DATA_DIR`
  - 启动桌面 sidecar
  - 等 `desktop-bootstrap.log` 出现 `preflight complete`
  - 断言 `certs/ca.crt` 存在
  - 断言日志包含 4 类结果之一
- 权限弹窗无法在 CI 自动化，用例只跑 headless 路径，弹窗路径走 human_tests。

### 真实场景测试

`human_tests/desktop-core-cert-bootstrap.md`：

- TC-DCB-01：默认 `~/.bifrost` 首次启动，弹权限、通过、`System.keychain` 存在证书、状态 InstalledAndTrusted。
- TC-DCB-02：自定义 `BIFROST_DATA_DIR=/tmp/bt-$$` 启动，CA/日志/traffic 全部落该目录。
- TC-DCB-03：用户取消权限弹窗，主窗口仍进入，日志含 `cancelled by user`。
- TC-DCB-04：删除 `certs/ca.crt` 后重启，重新生成并触发 GUI 安装。
- TC-DCB-05：sidecar 启动参数含 `--skip-cert-check`，非交互无终端 prompt。
- TC-DCB-06（macOS）：`security find-certificate -c "Bifrost CA" /Library/Keychains/System.keychain` 返回证书；`security find-certificate -c "Bifrost CA" ~/Library/Keychains/login.keychain-db` 不返回。

### 覆盖率与项目校验

- `cargo test -p bifrost-tls`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo build -p bifrost-desktop`（若存在）或桌面构建脚本
- 桌面 E2E 手工用例按 human_tests 记录
- 本地按 `rust-project-validate` 约定豁免 `make coverage`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：sidecar 非交互、延迟弹窗、System.keychain、失败降级、日志留痕、自定义 data_dir。
- 复核 diff：`desktop/src-tauri/src/main.rs`、`bifrost-tls/src/install.rs`、README。
- 重点 review：
  - `schedule_desktop_cert_ready` 是否只在 backend ready 之后被调用一次？
  - `--skip-cert-check` 是否有被误删的历史 diff？
  - `install_and_trust_gui` 失败时是否 panic？（必须不 panic，返回 Err）
- 复测：`bifrost-tls` 单测 + 手工 macOS 首次启动。

### 第 2 轮

- 检查 `git status --short`、`git diff` 无遗漏。
- 重点 review：日志格式是否稳定（human_tests 会 grep）；`UserCancelled` 分支能否被外部识别；自定义 data_dir 路径拼接无 `..` 越界。
- 复测：删除 CA 重启、cancel 场景、Windows/Linux 分支冒烟。

## 风险与决策点

- **延迟 2 秒是否过短**：如果 backend ready 早于窗口 init，2s 可能仍与 Tauri init 竞争。已观测稳定；若报白屏，可提高到 3~4s。
- **是否引入用户级 keychain 兜底**：拒绝。login keychain 的信任在其它 App 里不共享，会误导用户以为“安装成功”但 Chrome 仍报警。
- **UserCancelled 是否要主动重试**：不主动。用户明确拒绝，反复弹会激怒用户；改由设置页“重新安装”按钮触发。
- **`--skip-cert-check` 语义漂移**：CLI 未来若把此 flag 用于其他检查，桌面端需要独立 flag（如 `--desktop-managed`）。当前语义一致。
- **多桌面实例并发**：桌面端已单实例（macOS 应用注册 + `find_existing_backend_port` 复用），不会有两个壳层同时预检。
