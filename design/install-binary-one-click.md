# 二进制安装脚本一键体验

## 背景

`install-binary.sh` / `install-binary.ps1` 是 Bifrost 官方的 `curl ... | bash` / `irm ... | iex` 远程一键安装入口。历史上安装完二进制后，用户还需要手动执行「安装 CA -> 安装 skills -> 启动 daemon -> 调整系统 PATH」四步；对新用户体验极差、对 CI/自动化则容易漏步骤。同时下载路径有若干顽固痛点：

- 默认 GitHub 直连在受限网络下会拖到完整超时才失败，无任何反馈。
- 下载器（curl / wget / aria2c / axel）默认静默或多进度条相互打架。
- 二进制先复制到最终路径再改权限时，`bifrost --version` / `guardian` / post-install 可能命中「半成品 Mach-O」，在 macOS 上表现为 launch-suspended、dyld 卡死。
- `bifrost upgrade` 后 daemon 重启时经常撞 `EADDRINUSE`，或在 macOS 上因为二次 fork 触发 `objc_initializeAfterForkError`；Windows 长期返回 `daemon unsupported`。
- Debian 10 等旧 glibc 机器直接下 GNU 产物会因 `GLIBC_2.39` 缺失起不来。

本方案把上述痛点在同一次安装入口中修复，形成「一键安装完，用户已经拿到可用 Bifrost」的目标。高级用户和 CI 仍可通过参数 / 环境变量精细跳过。

## 用户目标验证清单

### 必须实现

- Bash / PowerShell installer 始终安装 CLI；macOS / Windows 下载完 CLI 后通过新二进制执行 `bifrost app install --version <同版本> --yes` 自动安装桌面 App，Linux 等无桌面资产的平台保持 CLI-only；桌面安装失败只 warn，不回滚已装好的 CLI。
- 桌面安装后自动执行 CA 安装、`install-skill --tool all -y`、`start --daemon --yes`；每步失败只 warn，不回滚已装好的 CLI / App。
- 下载源自适应：先并发探测 GitHub 直连与内置镜像（`ghfast.top` 等），选择最快返回的源；被选源完整下载失败后回退到全镜像竞速。
- 下载进度默认可见：`curl --progress-bar`、`wget --force-progress`、`aria2c --summary-interval=1`、`axel` 默认；并发竞速候选通过 `BIFROST_DOWNLOAD_PROGRESS=0` 静音。
- `bifrost upgrade` 复用同一套镜像探测和进度输出；替换二进制前先落地临时文件，再原子替换目标路径；旧二进制 backup 保留到新二进制 `--version` 校验和 musl fallback 全部收敛。
- `bifrost upgrade` 替换二进制后按 runtime.json 中记录的 HTTP / SOCKS5 端口、host、系统代理状态重启 daemon；runtime.json 缺失时按当前默认配置构造，legacy runtime 缺失系统代理字段时显式 `--no-system-proxy`，避免升级意外启用系统代理。
- 重启路径在 `stop_for_restart` 成功后必须等待 HTTP 和 SOCKS5 端口全部释放，10 秒内仍被占用先执行系统代理 crash recovery、清理 restart shutdown marker，再返回带 `netstat -ano | findstr :<port>` 诊断的明确错误，禁止直接 `start -d` 撞 `EADDRINUSE`。
- Windows 上当前运行的 `bifrost.exe` 不允许被同一进程直接替换；`bifrost upgrade` 检测到目标就是当前 exe 时提前给出「使用外部 PowerShell 安装脚本或 updater 进程」的明确错误。
- macOS / Windows 上 `start --daemon` 不再 fork 后继续初始化完整运行时。父进程改为通过 `std::process::Command` 启动当前二进制的 exec 子进程，注入 `BIFROST_DETACHED_DAEMON_CHILD=1`，绕过二次 daemon fork，直接跑前台启动路径但把 runtime 标记为 `daemon`。
- 端口 ready 探测把 `0.0.0.0` / `::` / `[::]` 归一到 `127.0.0.1`，避免 wildcard listener 被误判成不可连接。
- Bash installer 在 Windows Git Bash 环境下同步把安装目录写入 Windows User `Path`，让新开 PowerShell / CMD / Git Bash 都能直接执行 `bifrost`。
- PowerShell installer 兼容 Windows PowerShell 5.1：显式加载 `System.Net.Http`、不使用 PowerShell 7 才有的多参数 `Join-Path`、兼容 `PROCESSOR_ARCHITEW6432` / `PROCESSOR_ARCHITECTURE` 的 `ARM64` / `AMD64`；`Path` 写入采用「去重后前置」，让新装二进制优先于机器上已有旧 `bifrost.exe`。
- 每个 post-install 子命令默认有 `BIFROST_INSTALL_POST_INSTALL_TIMEOUT=120` 秒 watchdog，卡住不会永远占用用户终端。

### 必须不破坏

- CI / 自动化可用 `--no-post-install` / `BIFROST_INSTALL_POST_INSTALL=0` 完全跳过 post-install。
- CLI-only 环境可用 Bash / Git Bash 的 `--no-desktop`、PowerShell 本地脚本的 `-NoDesktop`，或跨入口通用的 `BIFROST_INSTALL_AUTO_DESKTOP=0` 跳过桌面 App；`--no-post-install` 只控制 CA / skills / daemon，不隐式跳过 App。
- 单步跳过：`--no-install-cert` / `BIFROST_INSTALL_AUTO_CERT=0`、`--no-install-skills` / `BIFROST_INSTALL_AUTO_SKILLS=0`、`--no-start` / `BIFROST_INSTALL_AUTO_START=0`。
- 用户显式设置的 `BIFROST_GITHUB_MIRROR` 仍作为最优先候选源。
- `BIFROST_DOWNLOAD_CONNECT_TIMEOUT` / `BIFROST_DOWNLOAD_TIMEOUT` / `BIFROST_DOWNLOAD_TRIES` / `BIFROST_MIRROR_PROBE_TIMEOUT` 环境变量继续控制下载。
- `--no-modify-path` 同时跳过 Git Bash `~/.bashrc` 和 Windows User `Path` 写入。
- 已 daemon 化的 Bifrost 服务在 upgrade 前后仍支持 `stop` / `status` / `restart`。

### 必须真实验证

- 通过 stub 网络探测函数模拟 GitHub 不可用，安装脚本自动选择 `https://ghfast.top/https://github.com`。
- 本地构造真实 upgrade 链路：旧版 `0.0.99` 二进制作为运行中的旧 daemon，用当前源码构建出的新二进制通过 debug-only hook 从本地 archive 完成升级、重启，新 daemon 用升级后的安装路径启动、Admin API ready、无 ObjC fork-safety crash、stop 后端口释放、无同数据目录 tray helper 残留。

## 产品语义

一键体验分为「安装 CLI -> 支持平台安装同版本桌面 App -> post-install 一键初始化 -> 升级/重启」四个阶段：

1. **安装二进制**：`install-binary.sh` / `install-binary.ps1` 下载 latest 或指定版本的 release 资产，解压到 `$INSTALL_DIR`（默认平台惯例目录），执行 checksum 校验。
2. **安装桌面 App**：macOS / Windows 用刚安装的 CLI 执行 `app install --version <CLI 版本> --yes`，复用 Rust CLI 中现有的目标平台判断、release 资产命名、DMG / MSI 安装和安装后版本校验；不支持的平台明确提示 CLI-only。
3. **post-install 初始化**（默认顺序，不可换序）：
   1. `bifrost ca install`
   2. `bifrost install-skill --tool all -y`
   3. `bifrost start --daemon --yes`
4. **`bifrost upgrade`**：复用镜像探测 / 进度 / 原子替换 / musl fallback；替换完成后按 runtime.json 精确重启 daemon，并在 macOS / Windows 上走 exec-child daemon 路径。

## 技术细节

### 下载源自适应

- Bash installer：`probe_github_url` 并发探测 GitHub 直连 + 镜像候选；`get_latest_version_via_redirect` 并发抢最快 latest 结果；`download_file` 使用探测出的最快源；失败时回退到 `download_github_file_race` 全镜像竞速。
- PowerShell installer：`Test-GithubUrl`、`Get-LatestVersionViaRedirect`、`Invoke-BifrostDownload` 提供等价能力；`Write-Progress` 显式反馈下载状态。
- 内置镜像候选例：`https://ghfast.top/https://github.com`；用户可通过 `BIFROST_GITHUB_MIRROR` 强制某个源为最优先。

### 原子替换二进制

- Bash / PowerShell installer 和 `bifrost upgrade` 都必须：
  1. 下载到同目录临时文件。
  2. 处理权限 + macOS xattr（`xattr -d com.apple.quarantine`）。
  3. `rename` 原子替换最终路径。
- `bifrost upgrade` 的旧二进制 backup 保留到「新二进制 `--version` 校验」和「musl fallback」全部收敛；任一失败恢复旧二进制，成功后清理 backup。
- Windows 上 `bifrost upgrade` 检测到目标 = 当前运行 exe 时提前失败并提示外部安装脚本。

### daemon exec-child 模型

- 父进程 `std::process::Command::new(current_exe).arg("start").arg("--daemon-child").env("BIFROST_DETACHED_DAEMON_CHILD", "1")`。
- macOS / Linux：`setsid()` 脱离终端；child stdout / stderr 重定向到 daemon log；`current_dir(&bifrost_dir)` 切换到 `BIFROST_DATA_DIR`。
- Windows：`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`。
- child 绕过二次 daemon fork，跑前台启动路径，但 runtime.json 仍写 `runtime_start_mode=daemon`。
- 父进程等待代理监听端口 ready 或 child 提前退出；ready 探测把 `0.0.0.0` / `::` / `[::]` 归一到 `127.0.0.1`。
- 该链路避免 macOS Objective-C runtime 在 fork 后首次初始化时触发 `+[NSNumber initialize]` / `objc_initializeAfterForkError`。

### 端口释放等待

- `wait_for_port_released(port, deadline)` 通过 bind 探测判断端口空闲；`wait_for_restart_port_release` 以 `cfg(any(unix, windows))` 编译，Unix / Windows 行为一致。
- 10 秒内仍被占用时输出 `netstat -ano | findstr :<port>`（Windows）或 `lsof -iTCP:<port> -sTCP:LISTEN`（Unix）诊断。

### CLI + Web + Admin API

CLI：

```bash
# 一键安装 + 初始化
curl -fsSL https://install.bifrost.dev/install-binary.sh | bash
irm https://install.bifrost.dev/install-binary.ps1 | iex

# 只装 CLI
curl ... | bash -s -- --no-desktop
export BIFROST_INSTALL_AUTO_DESKTOP=0 && curl ... | bash

# 关闭全部 post-install（不影响默认桌面安装）
curl ... | bash -s -- --no-post-install
BIFROST_INSTALL_POST_INSTALL=0 curl ... | bash

# 单步关闭
--no-install-cert / --no-install-skills / --no-start / --no-modify-path

# 手动升级
bifrost upgrade                    # 默认自动 restart daemon
bifrost upgrade --yes              # 自动确认已有 daemon 重启提示

# daemon
bifrost start --daemon --yes
bifrost stop
bifrost status
```

Admin API 与 Web UI 不参与安装本身；`bifrost upgrade` 期间会通过现有 Admin readiness 探测确认新 daemon Ready。

### 相关文件

- `install-binary.sh`
- `install-binary.ps1`
- `crates/bifrost-cli/src/commands/upgrade.rs`
- `crates/bifrost-cli/src/commands/ca.rs`
- `crates/bifrost-cli/src/commands/install_skill.rs`
- `crates/bifrost-cli/src/commands/start.rs`
- `crates/bifrost-cli/src/main.rs`
- `crates/bifrost-cli/tests/daemon_shutdown.rs`
- `.github/workflows/ci.yml` / `release.yml`
- `e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `e2e-tests/tests/test_install_binary_windows_path.sh`
- `e2e-tests/tests/test_install_binary_post_install.sh`
- `e2e-tests/tests/test_upgrade_restart_e2e.sh`
- `e2e-tests/tests/test_upgrade_local_restart_e2e.sh`
- `e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`
- `e2e-tests/tests/test_install_musl_fallback.sh`
- `human_tests/install-binary-one-click.md`
- `human_tests/cli-start-stop-status.md`
- `human_tests/cli-tray-helper.md`

## Sync 边界

- installer / upgrade 与 Bifrost sync 服务无直接耦合；`stop_for_restart` 会先向 active worker 进度通道写入「Bifrost 正在重启或关闭」失败终态，worker stdout BrokenPipe 被降级为管道关闭，IM 长任务默认 2 次无输出心跳后把控制权还给模型但不终止底层 exec session。
- upgrade 完成后 `runtime.json` 记录 `runtime_start_mode=daemon`；下一次 `bifrost status` / `stop` 通过 runtime.json 定位 PID 与端口，避免依赖 `current_exe()`。

## Phase 1-4

### Phase 1：下载与安装底座

- 镜像探测 / 最快源选择 / 全镜像竞速回退。
- 原子替换二进制、macOS xattr 处理、Windows 当前 exe 保护。
- 默认下载进度可见 + 并发候选静音。

### Phase 2：post-install 一键化

- Bash / PowerShell installer 默认执行 `ca install` -> `install-skill --tool all -y` -> `start --daemon --yes`。
- 全局 / 分步 opt-out。
- 每子命令 120 秒 watchdog；子命令失败仅 warn 并给出重试命令。
- Windows Git Bash + PowerShell 双入口写入 Windows User `Path`，去重前置。

### Phase 3：`bifrost upgrade` 与 daemon exec-child

- upgrade 复用镜像探测与进度输出。
- 显式设置 `BIFROST_GITHUB_MIRROR` 时，该地址始终保持第一优先级，不参与内置候选的延迟竞速；内置候选只作为确定性 fallback，并行 coverage 负载不得改变顺序。
- 二进制 `--version` 校验带硬超时；Linux 刚完成二进制替换后若 spawn 瞬态返回 `ETXTBSY`，仅对该错误执行最多 8 次、总退避不超过 140ms 的有界重试，其他启动错误立即返回。
- runtime.json 精确重启：端口、host、系统代理策略；缺失时按默认配置回退，legacy 缺字段显式 `--no-system-proxy`。
- `stop_for_restart` -> `wait_for_restart_port_release` -> `start -d`；端口仍被占用时系统代理 crash recovery + shutdown marker 清理 + 明确诊断错误。
- macOS / Windows `start --daemon` 走 exec-child 模型。

### Phase 4：跨平台兼容与文档

- PowerShell 5.1 兼容性：显式加载 `System.Net.Http`、不使用 PS7 特性、`ARM64` / `AMD64` 架构映射、Path 去重前置。
- Linux glibc 旧机器自动走 musl fallback（详见 `design/linux-musl-install-fallback.md`）。
- 更新 `README.md`、`docs/getting-started.md`、`site/src/content/docs/getting-started/installation.mdx`、`human_tests/readme.md` 索引。

## 测试方案

### 单元测试（`crates/bifrost-cli/src/commands/upgrade.rs`）

真实存在的测试（`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_` / `env_flag_enabled` / `wait_for_port_released` 全部覆盖）：

- `test_detect_install_method_returns_valid_variant`
- `test_install_method_display`
- `test_cli_upgrade_rejects_restart_flag`
- `test_cli_upgrade_hidden_yes_flag_is_accepted`
- `test_cli_upgrade_no_flags`
- `upgrade_download_progress_formats_percent_and_size`
- `upgrade_github_path_url_joins_mirror_and_release_path`
- `upgrade_mirror_display_name_hides_full_path`
- `upgrade_download_tuning_parses_positive_values` / `upgrade_download_tuning_rejects_invalid_values`
- `upgrade_install_binary_atomically_replaces_existing_target`
- `upgrade_restore_binary_backup_restores_previous_target`
- `upgrade_command_status_with_timeout_reports_success_and_failure`
- `upgrade_command_status_with_timeout_does_not_block_on_hung_child`
- `wait_for_port_released_returns_quickly_when_port_is_free` / `wait_for_port_released_times_out_when_port_is_held`
- `upgrade_restart_port_from_runtime_defaults_to_9900` / `upgrade_restart_ports_from_runtime_uses_runtime_ports`
- `env_flag_enabled_accepts_true_values` / `env_flag_enabled_rejects_absent_and_false_values`
- `test_build_restart_args_no_runtime_info_uses_default_config_system_proxy` / `test_build_restart_args_no_runtime_info_preserves_disabled_default_config_system_proxy`
- `test_build_restart_args_with_runtime_info`
- `test_glibc_2_38_requires_musl_for_upgrade` / `test_glibc_2_39_keeps_gnu_for_upgrade` / `test_unknown_glibc_requires_musl_for_upgrade`
- `detached_daemon_readiness_host_maps_wildcard_listeners_to_loopback`
- `bash -n install-binary.sh` 覆盖 shell 语法。

### E2E 测试

- `e2e-tests/tests/test_install_binary_adaptive_download.sh`：镜像探测 / 最快源 / 全镜像回退 / 下载进度 / 静音候选 / `BIFROST_MIRROR_PROBE_TIMEOUT` help 暴露。
- `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`：PowerShell 等价路径 + Windows User `Path` 去重前置 + PS 5.1 兼容性。
- `e2e-tests/tests/test_install_binary_windows_path.sh`：Git Bash + Windows PATH 双写；`--no-modify-path` 同时跳过。
- `e2e-tests/tests/test_install_binary_post_install.sh`：dry-run 顺序 = `ca install` -> `install-skill --tool all -y` -> `start --daemon --yes`；分步 opt-out；120 秒 watchdog；`install_binary_atomically` 临时文件生命周期。
- `e2e-tests/tests/test_upgrade_restart_e2e.sh`：无 daemon / 已最新 / help 文案；源码门禁确认 `wait_for_restart_ports_release`、HTTP/SOCKS5 多端口、端口占用错误文案、`find_process_on_port`、系统代理恢复；Windows `cfg(any(unix, windows))` 门禁；macOS/Windows daemon exec-child + `BIFROST_DETACHED_DAEMON_CHILD` + `setsid()` + Windows detached flags + `current_dir(&bifrost_dir)` + wildcard host 归一 + main daemon bypass；stop 后 tray helper 无残留。
- `e2e-tests/tests/test_upgrade_local_restart_e2e.sh`：本地 0.0.99 旧 daemon + 本地 archive + debug-only hook；断言四个里程碑（检测运行中代理 / 停止旧代理 / 等待端口释放 / 重启成功）+ PID 变化 + 新安装路径 + Admin API ready + 无 ObjC crash + `--no-system-proxy` 模式保留。
- `e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`：通过 Admin API 触发 upgrade 后 restart 收敛。
- `e2e-tests/tests/test_install_musl_fallback.sh`：Debian 10 sandbox 走 musl。

### 真实场景测试

- `human_tests/install-binary-one-click.md`：默认镜像自适应、Windows 镜像自适应、下载回退、临时目录真实安装（`BIFROST_INSTALL_DIR=$(mktemp -d) --no-post-install --no-modify-path`）、命令顺序、全局 / 分步 opt-out、help 可发现、下载进度可见、竞速静音、原子替换、`bifrost upgrade` 最快源 / 进度 / env 超时 / 二进制校验硬超时、restart 端口释放 + 明确诊断、runtime.json 缺失回退、macOS/Windows exec-child + wildcard 归一、本地真实 upgrade restart 链路、IM/外部 Runner worker 优雅收口。
- `human_tests/cli-start-stop-status.md`：新增 macOS daemon exec-child 回归用例，临时数据目录启动真实 daemon，start/status/stop 正常、`runtime.json` 标记 daemon、日志无 `+[NSNumber initialize]` / `objc_initializeAfterForkError`、stop 后无同数据目录 tray helper。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 用户目标覆盖：安装进度、upgrade 进度、最快源并发、restart 端口释放、macOS ObjC 崩溃规避、post-install 顺序 / opt-out。
- 变更范围：`git status --short` / `git diff` 未触碰 im-gateway 无关模块。
- 代码 review：`install-binary.sh` 下载器参数、竞速静音、PATH 未刷新、权限失败、CI opt-out、dry-run；`bifrost upgrade` 镜像探测不破坏 `BIFROST_GITHUB_MIRROR`、被选源失败回退、PowerShell 与 Bash env 一致、restart 不在端口占用时启动、失败恢复系统代理；macOS daemon exec-child 无二次 fork、data dir 保留、setsid、runtime.json daemon 标记、stop 能力。
- 复测命令：
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib env_flag_enabled`
  - `bash -n install-binary.sh`
  - `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`
  - `bash e2e-tests/tests/test_install_binary_post_install.sh`
  - `bash e2e-tests/tests/test_upgrade_restart_e2e.sh`
  - `pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）

### 第 2 轮

- 对照第 1 轮 diff 与测试输出复核；确认文档、E2E、human_tests、README、Cargo.lock 同步。
- 失败路径重跑：`bash e2e-tests/tests/test_upgrade_local_restart_e2e.sh`、`bash e2e-tests/tests/test_install_musl_fallback.sh`、human_tests 中列出的真实 daemon 命令。
- 关注：Windows PowerShell 5.1 兼容性、Path 去重前置、`--no-modify-path` 双跳过。

## 校验要求

- `grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml`
- `grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml`
- `grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml`
- `bash -n install-binary.sh`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib env_flag_enabled`
- `cargo build --bin bifrost` 后使用临时数据目录执行 macOS daemon start/status/stop 真实场景验证。
- `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `bash e2e-tests/tests/test_install_binary_windows_path.sh`
- `bash e2e-tests/tests/test_install_binary_post_install.sh`
- `bash e2e-tests/tests/test_upgrade_restart_e2e.sh`
- `bash e2e-tests/tests/test_upgrade_local_restart_e2e.sh`
- `bash e2e-tests/tests/test_install_musl_fallback.sh`
- `cargo fmt --all -- --check`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `scripts/ci/local-ci.sh`：脚本和文档变更可按成本评估执行；未执行需在交付中说明。

## 风险与决策

- Post-install 失败仅 warn 不回滚 CLI，是刻意选择：证书信任受系统权限影响，rollback 反而会导致 CLI 二进制被误删。用户可根据 warn 中给出的重试命令自行修复。
- macOS `objc_initializeAfterForkError` 是 Apple 平台层限制，因此 `start --daemon` 走 exec-child 而非 fork-and-init；这一改动同时把 Windows daemon 从 `unsupported` 升级为一等公民。
- Windows 上无法在同一进程替换当前运行的 `bifrost.exe`；提前给出明确错误，避免用户在 `remove_file` 阶段看到模糊的 permission denied。
- 镜像自适应对企业内 SNI 阻断、TLS 中间人策略下的默认路径影响未知；用户可通过 `BIFROST_GITHUB_MIRROR` 强制指定源。
- 二进制 `--version` 硬超时是安全默认；Linux `ETXTBSY` 只做短时有界重试，持续占用与其他错误仍会触发旧二进制回滚；若新二进制真的启动极慢（如首次触发 macOS Gatekeeper 网络校验），可通过完整 upgrade 重试路径继续。
- `bifrost upgrade` 的 restart 端口等待 10 秒硬编码；若未来有系统内核层面延迟释放的场景，可参数化，但不会默认放大。
