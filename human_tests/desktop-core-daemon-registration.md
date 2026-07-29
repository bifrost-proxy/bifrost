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
3. 检查脚本退出码为 0，且输出包含 `PASS: Desktop ownership stays scoped by data-dir, transient stalls preserve PID, real exits recover, and CLI lifecycle commands preserve App ownership`。

预期结果：

- 脚本仅使用临时数据目录和动态端口，不停止或修改默认数据目录中的正式 Service。
- 即使 Desktop 父环境带有 `BIFROST_DETACHED_DAEMON_CHILD=1`，Desktop sidecar 的 `runtime.json` 仍记录 `runtime_start_mode=desktop`。
- 通过 Desktop 单实例 graceful shutdown 通道退出后，Desktop-owned Service PID 在有界时间内退出。
- CLI `start --daemon` 创建的 Service 记录 `runtime_start_mode=daemon`；Desktop 复用该 Service 后退出，原 CLI Service PID 和健康端点仍保持可用。
- 脚本最后通过 CLI `stop` 清理临时 daemon，并删除临时数据目录。

### TC-DCDR-06（回归）：Desktop 不复用其他 data-dir 的相邻端口 Service

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
3. 检查脚本退出码为 0，且最终输出包含
   `PASS: Desktop ownership stays scoped by data-dir, transient stalls preserve PID, real exits recover, and CLI lifecycle commands preserve App ownership`。

预期结果：

- 脚本先在临时 foreign data-dir 的相邻动态端口启动 CLI daemon，再使用另一个临时
  data-dir 启动 Desktop。
- Desktop 不把相邻端口的健康 foreign Service 当作自己的 backend，而是在自己的配置端口
  启动独立 `runtime_start_mode=desktop` Service。
- Desktop 退出后只停止自己 data-dir 的 Desktop-owned Service，foreign Service PID 仍存活；
  最后由脚本使用 foreign data-dir 定向停止。
- 整个用例不读取、停止或替换默认 data-dir 的正式 Service，也不固定使用 `9900/9901`。

### TC-DCDR-07（回归）：用户 CLI stop/restart 不改变 Desktop ownership

操作步骤：

1. 执行真实进程生命周期脚本：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh
   ```
2. 检查脚本在 Desktop-owned Service 运行期间分别执行用户 CLI `stop` 与 `restart`，两条命令
   都非零退出且输出包含 `owned by the Desktop app`。
3. 检查命令执行后的 `runtime.json` PID 与 `runtime_start_mode`，再通过 Desktop 单实例
   graceful shutdown 通道退出 App。

预期结果：

- 用户 CLI `stop` 和 `restart` 都 fail-closed；即使 `restart` 具备 daemon 重启语义，也不能
  把 live Desktop-owned Service 静默转换为 CLI daemon。
- 两条 CLI 命令之后，原 Service PID 继续存活，`runtime_start_mode` 仍为 `desktop`。
- 如果 Desktop marker 的 PID 已被系统复用且进程启动时间不匹配，则该 marker 视为 stale，
  不会错误阻断 CLI；没有启动时间字段的历史 marker 继续兼容 PID-only 判断。
- Desktop 自己的 graceful shutdown 使用内部授权 stop，仍能退出 App 并停止其拥有的
  Service；日志包含 `desktop shutdown owns the active backend; requesting backend stop`。
- CLI-owned daemon 被 Desktop 复用并退出 App 时仍保持运行，确保新门禁不破坏既有 owner
  语义。

### TC-DCDR-08（回归）：Tray 操作与 Service owner 一致

操作步骤：

1. 执行 Tray ownership focused 单元测试：
   ```bash
   cargo test -p bifrost-cli commands::tray::runtime::tests --lib -- --nocapture
   cargo test -p bifrost-cli commands::tray::menu::tests --lib -- --nocapture
   cargo test -p bifrost-cli desktop_shutdown_request_accepts_only_desktop_shell_executables --lib -- --nocapture
   cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_backend_stop_command_is_authorized_for_owned_runtime -- --nocapture
   ```
2. 准备当前 debug CLI sidecar 与 Desktop binary：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   node scripts/prepare-tauri-sidecar.mjs debug
   SKIP_FRONTEND_BUILD=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
   ```
3. 执行真实 ownership 生命周期：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh
   ```

预期结果：

- Tray 解析 `runtime_start_mode=desktop` 后，主操作显示 `Quit Bifrost`，action 为
  `QuitDesktop`；该操作不依赖 CLI binary availability。
- Tray 解析 `daemon`、`foreground`、缺失字段的历史 `unknown` runtime 后，运行态主操作
  仍为 `Stop Bifrost`，停止后仍为 `Start Bifrost`。
- `QuitDesktop` 只接受运行中 Service 的直系父进程且可执行文件名为
  `bifrost-desktop` / `bifrost-desktop.exe`，然后发送
  `--bifrost-upgrade-shutdown`；执行前校验菜单记录的 Service PID 启动时间，拒绝 PID
  复用，不直接 kill Desktop 或 Service。
- Desktop graceful shutdown 的异步 stop helper 与同步 restart-stop 共用 command
  配置，必须带 `BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL=1`；普通用户 CLI
  `stop/restart` 仍不带该授权并继续 fail-closed。
- Desktop-owned 场景中 App 和 owned Service 均退出；再等待 3 秒（超过一次 2 秒
  watchdog poll）后原 PID 与健康端点仍未恢复。
- CLI-owned daemon 被 Desktop 复用后，Desktop 退出但 Service 保持健康，最后仍可由
  CLI `stop` 正常停止。

### TC-DCDR-09（回归）：短时健康探针失败不重启，真实子进程退出快速恢复

操作步骤：

1. 准备当前 debug CLI sidecar 与 Desktop binary：
   ```bash
   pnpm --dir web run build:desktop
   SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
   node scripts/prepare-tauri-sidecar.mjs debug
   SKIP_FRONTEND_BUILD=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
   ```
2. 执行 watchdog focused 单元测试：
   ```bash
   cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog_ -- --nocapture
   ```
3. 执行真实 ownership 生命周期脚本：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh
   ```
4. 检查脚本退出码为 0，且最终输出包含
   `transient stalls preserve PID, real exits recover`。

预期结果：

- 测试仅使用临时 data-dir、动态端口、`BIFROST_DESKTOP_NO_SYSTEM_PROXY=1` 和
  `BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1`，不读取或修改正式 `9900` Service、系统代理或证书。
- 对 Desktop-owned Service 发送 `SIGSTOP` 并保持 12 秒后再 `SIGCONT`，健康日志记录
  `desktop backend health degraded` 和 `desktop backend health recovered without restart`，
  `runtime.json` 中 PID 始终不变。
- 随后使用 `SIGKILL` 真实终止该子进程；Desktop watchdog 在下一轮 2 秒存活检查中记录
  `managed backend child pid=... exited`，拉起不同 PID，并恢复健康端点。
- 后续用户 CLI `stop/restart` 仍拒绝操作 Desktop-owned Service，Desktop graceful shutdown
  仍停止当前 owned PID；CLI-owned Service 在 Desktop 退出后仍保持运行。

### TC-DCDR-10（回归）：App/Tray 整组退出顺序与 orphan Tray 恢复

操作步骤：

1. 执行 Tray orphan owner focused tests：
   ```bash
   cargo test -p bifrost-cli orphan_desktop_stop --lib -- --nocapture
   cargo test -p bifrost-cli ordinary_tray_stop_command_does_not_gain_desktop_authorization --lib -- --nocapture
   ```
2. 执行 Desktop Quit focused tests：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_quit_ -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_backend_stop_command_is_authorized_for_owned_runtime -- --nocapture
   ```
3. 在没有正式 `bifrost-desktop` 进程的 macOS 会话执行真实 ownership 生命周期：
   ```bash
   SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh
   ```
4. 检查临时 data-dir 的 `desktop-bootstrap.log`：必须先出现
   `backend stop helper completed successfully; owned backend and tray are stopped`，后出现
   `desktop lifecycle group shutdown complete; requesting final app exit`。

预期结果：

- Desktop 自己退出 App-owned 模式时，在后台等待内部 stop helper 成功；该 helper 停止
  core 和 Tray 后 Desktop 才执行最终 `app.exit(0)`。stop helper 失败或超时不会让
  Desktop 先退出，窗口恢复且允许重试。
- Tray 的 `Quit Bifrost` 在 Desktop 正常时请求 Desktop graceful shutdown 并退出自身；
  Desktop 已异常消失时，只对 `runtime_start_mode=desktop`、PID、菜单快照启动时间和系统
  观察启动时间全部精确匹配的同一实例执行内部授权 stop。缺失启动时间、PID 复用、
  daemon/foreground/unknown owner 或 runtime 缺失都 fail-closed。
- orphan fallback 命令同时携带 `BIFROST_TRAY_INVOKED_STOP=1` 与
  `BIFROST_DESKTOP_AUTHORIZED_STOP_INTERNAL=1`；普通 Tray stop 显式移除 Desktop
  授权，Desktop 自己的 stop 显式移除 Tray-preserve 标记，确保 App Quit 会停止 Tray。
- 用户直接运行 CLI `stop/restart` 仍拒绝 live Desktop-owned Service；Desktop 复用
  CLI-owned daemon 后退出时仍保留该 Service。
- 测试仅使用临时 data-dir 和动态端口；如果检测到正式 Desktop 进程则安全跳过真实
  单实例 E2E，禁止向正式 App 投递退出请求。

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
| 2026-07-25 | TC-DCDR-06 / 07 回归 | `SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`；执行前 `pgrep` 识别到正式 `/Applications/Bifrost.app/Contents/MacOS/bifrost-desktop` PID `58982`。 | 环境阻塞：脚本按安全门禁输出 `SKIP: an existing Bifrost Desktop process is running`，未停止正式 App，也未触碰正式 `9900/9901` Service。新增跨 data-dir 与 CLI `stop/restart` 真实断言尚需在无正式 Desktop 进程的 macOS CI/会话补跑。 |
| 2026-07-27 | TC-DCDR-08 回归 | 先执行 Tray runtime `6/6`、menu `27/27`、Desktop shell path `1/1`、Desktop stop command `1/1` focused tests；再构建当前 CLI/Desktop 并执行 `SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`。首次用旧 `0.0.164` Desktop binary 被相邻端口旧行为阻断；重编 `0.0.165` 后又真实发现异步 `spawn_backend_stop` 漏传 Desktop 私有授权，stop helper 输出 `owned by the Desktop app`。将同步/异步 stop 收敛到 `configure_backend_stop_command` 后按本用例立即全量复跑；第 1 轮 review 继续补上 Service 启动时间校验和 `foreground/daemon/unknown` 菜单矩阵并再次复跑；第 2 轮复查执行 Tray 模块 `155/155`、Desktop crate `65/65`，随后 `cargo fmt --all -- --check`、全目标全 feature clippy/build、`cargo test --workspace --all-features` 均通过。远端 CI 的 macOS agent-extensions shard 连续两次表现为 App Server 成功而 3 秒 `traex --version` 探针未生成 `cli.version`，同一 head 本地精确脚本通过；将非关键版本探针预算提高到 10 秒并保留严格 metadata 断言后再次复跑。后续 proxy-core shard 的 mock server 实际打印 `READY`，但重定向文件因 Python stdout buffering 直到退出才可见；改为 `python3 -u` 后精确复跑 cleanup E2E，`1 passed, 0 failed`。 | 通过：Desktop runtime 显示 `Quit Bifrost` 并请求 Desktop graceful shutdown，CLI runtime 保持 `Stop/Start`；异步 owned stop 获得内部授权，普通 CLI stop/restart 保护不变；陈旧菜单 PID 复用会被拒绝；Desktop-owned App/Service 均退出且 3 秒后未被 watchdog 恢复，CLI-owned daemon 在 Desktop 退出后保持健康并由 CLI 定向清理。 |
| 2026-07-28 | TC-DCDR-09 回归 | 生成当前 debug CLI sidecar 与 Desktop binary；执行 `cargo test --manifest-path desktop/src-tauri/Cargo.toml desktop_watchdog_ -- --nocapture` 和 `SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`。E2E 初版在 startup recovery gate 结束前暂停 core，未进入运行期 watchdog；增加 `desktop backend start succeeded` 门禁后重跑。 | 通过：focused 单测 `4/4`；真实 Desktop-owned core 暂停 12 秒后恢复且 PID 不变，随后真实退出被拉起为新 PID；CLI stop/restart、Desktop graceful shutdown 与 CLI-owned Service 保留回归均通过。测试只使用临时 data-dir、动态端口并禁用系统代理。 |
| 2026-07-29 | TC-DCDR-10 回归（本地首轮） | 创建/更新用例后立即执行两组 Tray focused tests、两组 Desktop focused tests及 ownership E2E。 | 部分通过：Tray orphan `2/2`、普通 Tray stop 授权隔离 `1/1`、Desktop Quit helper `2/2`、Desktop stop command `1/1` 均通过；真实 E2E 检测到正式 `/Applications/Bifrost.app` 的 Desktop PID `10981` 后按安全门禁跳过，未向正式 App 投递退出请求。完整真实链路待无正式 Desktop 的 macOS CI 补跑。 |
| 2026-07-29 | TC-DCDR-10 回归（本地完整复跑） | 正式 Desktop 退出后确认系统中无 `bifrost-desktop` 进程，执行 `bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`；脚本重新构建当前 WebUI、CLI sidecar 和 Desktop，并检查 `desktop-bootstrap.log` 中 owned stop helper 完成早于 Desktop 最终退出。 | 通过：真实 Desktop-owned App/Core/Tray 按组退出且未被 watchdog 恢复，普通 CLI stop/restart 继续拒绝 App-owned Service；CLI-owned daemon 在 Desktop 退出后 PID 与健康端点保持可用。脚本退出码 0，全部使用临时 data-dir、动态端口并禁用系统代理，未触碰正式 9900 Service。 |
