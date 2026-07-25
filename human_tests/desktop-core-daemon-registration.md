# Desktop Core LaunchDaemon Registration Boundary

## 功能模块说明

验证桌面 app 启动的 app-bound core 不注册或升级 macOS system-proxy cleanup LaunchDaemon，避免覆盖 CLI daemon 的系统级注册。CLI daemon 路径仍保留自己的 LaunchDaemon 注册能力，并且 CLI `start` 识别 live Desktop core 后不能为了重启自己的服务而误 stop app-bound core。Desktop 创建 sidecar 时还必须清除从父进程继承的 detached-daemon 内部标记，确保 Desktop-owned Service 随 Desktop 退出，而既有 CLI-owned Service 在 Desktop 退出后继续运行。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 使用临时数据目录，避免污染本机真实配置：
  ```bash
  export BIFROST_DATA_DIR="$(mktemp -d)"
  export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  export BIFROST_DISABLE_TRAY=1
  export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
  ```
- 本用例不执行 `sudo`、不写 `/Library/LaunchDaemons/`，只验证桌面 sidecar 启动契约与 CLI 边界。

## 测试用例列表

### TC-DCDR-01：桌面 sidecar 环境禁用 LaunchDaemon 注册

操作步骤：

1. 执行 focused 单元测试：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar_disables_launchd_cleanup_registration -- --nocapture
   ```
2. 检查测试输出为通过。

预期结果：

- `desktop_backend_env` 包含 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`。
- `desktop_backend_env` 包含 `BIFROST_DESKTOP_CORE=1`，使 sidecar 写出 `runtime_start_mode=desktop`。
- `desktop_backend_env` 同时包含当前 Desktop sidecar 使用的 `BIFROST_DATA_DIR`。
- 测试不启动真实桌面窗口，不触碰真实系统代理或 LaunchDaemon。

### TC-DCDR-02：桌面系统代理开关与 LaunchDaemon 注册抑制相互独立

操作步骤：

1. 执行 focused 单元测试：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar_start_args_keep_system_proxy_policy_separate_from_launchd_registration -- --nocapture
   ```
2. 检查测试输出为通过。

预期结果：

- 未设置 `BIFROST_DESKTOP_NO_SYSTEM_PROXY` 时，桌面 sidecar 参数不包含 `--no-system-proxy`。
- 因为未传 `--no-system-proxy`，core 仍读取用户配置中的系统代理开关；新配置默认 `system_proxy.enabled=true`，只有用户配置或显式环境禁用时才不启动系统代理配置。
- 设置 `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1` 时，桌面 sidecar 参数包含 `--no-system-proxy`。
- 两种情况下 LaunchDaemon 注册抑制都由 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1` 承担，不依赖 `--no-system-proxy`。

### TC-DCDR-03：CLI 识别 Desktop core 并避免误 stop

操作步骤：

1. 执行 focused CLI 单元测试：
   ```bash
   cargo test -p bifrost-cli desktop_core --lib -- --nocapture
   cargo test -p bifrost-cli live_desktop_runtime --lib -- --nocapture
   ```
2. 执行 Desktop runtime restartability 单元测试：
   ```bash
   cargo test -p bifrost-cli runtime_info_new_desktop_is_app_bound_not_cli_restartable --lib -- --nocapture
   ```

预期结果：

- `BIFROST_DESKTOP_CORE=1` 被映射为 `RuntimeStartMode::Desktop`。
- detached daemon child 优先级高于 Desktop env，不破坏 CLI daemon 子进程语义。
- CLI `start` 遇到同端口 live Desktop runtime 时复用并返回成功。
- CLI `start` 遇到不同端口 live Desktop runtime 时返回清晰错误，错误包含 `will not stop the app-bound core`。
- `RuntimeStartMode::Desktop` 不被 CLI managed-runtime helper 视为可重启 daemon。

### TC-DCDR-04：E2E 合约脚本覆盖桌面 sidecar 注册边界和 CLI ownership 边界

操作步骤：

1. 执行 E2E 合约脚本：
   ```bash
   bash e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh
   ```
2. 检查脚本退出码为 0。

预期结果：

- 脚本运行 `desktop_sidecar` focused tests 并通过。
- 脚本运行 CLI `desktop_core` ownership tests 并通过。
- 脚本不会安装、卸载或修改 `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist`。
- CLI 的 `spawn_system_proxy_launchd_install_task` 仍保留环境变量门禁，说明 CLI 注册路径未被删除。

### TC-DCDR-05（回归）：Desktop-owned Service 随 Desktop 退出且 CLI-owned Service 保留

操作步骤：

1. 在有 macOS WindowServer 会话的机器上准备 debug CLI sidecar 与 Desktop binary：
   ```bash
   pnpm --dir web run build:desktop
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   node scripts/prepare-tauri-sidecar.mjs debug
   SKIP_FRONTEND_BUILD=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
   ```
2. 执行真实进程生命周期脚本：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh
   ```
3. 检查脚本退出码为 0，且输出包含 `PASS: Desktop-owned Service exits with Desktop while CLI-owned Service is preserved`。

预期结果：

- 脚本仅使用临时数据目录和动态端口，不停止或修改默认数据目录中的正式 Service。
- 即使 Desktop 父环境带有 `BIFROST_DETACHED_DAEMON_CHILD=1`，Desktop sidecar 的 `runtime.json` 仍记录 `runtime_start_mode=desktop`。
- 通过 Desktop 单实例 graceful shutdown 通道退出后，Desktop-owned Service PID 在有界时间内退出。
- CLI `start --daemon` 创建的 Service 记录 `runtime_start_mode=daemon`；Desktop 复用该 Service 后退出，原 CLI Service PID 和健康端点仍保持可用。
- 脚本最后通过 CLI `stop` 清理临时 daemon，并删除临时数据目录。

## 清理步骤

```bash
rm -rf "$BIFROST_DATA_DIR"
```

## 执行记录

| 日期 | 用例 | 执行命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-09 | TC-DCDR-01 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar -- --nocapture`；测试 `desktop_sidecar_disables_launchd_cleanup_registration` 通过。 | 通过：Desktop sidecar env 同时包含 `BIFROST_DATA_DIR`、`BIFROST_DESKTOP_CORE=1` 和 `BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1`，未触碰真实 LaunchDaemon。 |
| 2026-07-09 | TC-DCDR-02 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar -- --nocapture`；测试 `desktop_sidecar_start_args_keep_system_proxy_policy_separate_from_launchd_registration` 通过。 | 通过：默认 args 不包含 `--no-system-proxy`，保留按用户配置启用系统代理；设置 `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1` 后才添加 `--no-system-proxy`。 |
| 2026-07-09 | TC-DCDR-03 | `cargo test -p bifrost-cli desktop_core --lib -- --nocapture`、`cargo test -p bifrost-cli live_desktop_runtime --lib -- --nocapture`、`cargo test -p bifrost-cli runtime_info_new_desktop_is_app_bound_not_cli_restartable --lib -- --nocapture`。 | 通过：Desktop env 映射为 `RuntimeStartMode::Desktop`；detached daemon 优先级不变；同端口 live Desktop runtime 被复用；不同端口返回包含 `will not stop the app-bound core` 的错误；Desktop runtime 不可被 CLI managed helper 重启。 |
| 2026-07-09 | TC-DCDR-04 | `bash e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh`。 | 通过：脚本串行执行 Desktop sidecar、CLI desktop ownership、live Desktop runtime 和 Desktop restartability focused tests，退出码 0；未安装、卸载或修改 `/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist`。 |
| 2026-07-09 | TC-DCDR-04 CI 回归 | PR #361 CI run `28995925917` / job `86052744074` 失败样本显示 Linux shell CI 中 `test_desktop_sidecar_launchd_env_contract.sh` 因缺少 `glib-2.0.pc` 触发 `gio-sys v0.18.1` build script 失败；修复后执行 `bash -n e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh`、`bash scripts/ci/check-e2e-shell-ci-coverage.sh`、`bash e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh`。 | 通过：脚本在 desktop-capable 本机先准备 `web/dist-desktop`、debug CLI sidecar 与 `desktop/src-tauri/resources/bin/*`，再运行 Desktop sidecar focused tests 和 CLI ownership focused tests；Linux 缺 GTK/GObject 开发包时只跳过 desktop crate 部分，CLI Desktop ownership 边界仍会执行。 |
| 2026-07-25 | TC-DCDR-01 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar_disables_launchd_cleanup_registration -- --nocapture`。 | 通过：1 passed；Desktop sidecar 的 LaunchDaemon 注册抑制环境保持有效。 |
| 2026-07-25 | TC-DCDR-02 | `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_sidecar_start_args_keep_system_proxy_policy_separate_from_launchd_registration -- --nocapture`。 | 通过：1 passed；系统代理参数和 LaunchDaemon 注册抑制仍相互独立。 |
| 2026-07-25 | TC-DCDR-03 | `cargo test -p bifrost-cli desktop_core --lib -- --nocapture`、`cargo test -p bifrost-cli live_desktop_runtime --lib -- --nocapture`、`cargo test -p bifrost-cli runtime_info_new_desktop_is_app_bound_not_cli_restartable --lib -- --nocapture`。 | 通过：分别 2、4、1 passed；detached daemon 优先级保持，CLI 不误 stop Desktop runtime。 |
| 2026-07-25 | TC-DCDR-04 | `bash e2e-tests/tests/test_desktop_sidecar_launchd_env_contract.sh`。 | 通过：Desktop sidecar 4 passed、CLI ownership 2 passed、live Desktop runtime 4 passed、restartability 1 passed；静态合约确认 Desktop sidecar 清除继承的 daemon marker。 |
| 2026-07-25 | TC-DCDR-05 回归 | `SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`。 | 通过：污染父环境时 Desktop sidecar 仍记录 `desktop` 并随 Desktop graceful shutdown 退出；CLI daemon 被 Desktop 复用后，Desktop 退出但原 PID 与健康端点保持可用，最后由测试定向清理。 |
