# 二进制安装脚本一键体验

## 功能模块说明

`install-binary.sh` 是用户通过 `curl ... | bash` 使用的官方远程二进制安装入口。安装完成后，脚本默认继续完成一键体验初始化：

- 安装并信任 Bifrost CA 证书。
- 安装所有支持 AI 工具的 Bifrost skills。
- 启动 Bifrost 服务，用户安装完成后可直接访问默认管理端和代理能力。

目标是把原先“安装二进制后再手动处理证书、skill、启动服务”的多步流程合并为一次安装命令。高级用户和 CI 仍可通过参数或环境变量跳过自动步骤。本轮同时修复二进制下载长时间静默的问题，并让 `bifrost upgrade` 复用安装脚本的最快源选择能力；针对 upgrade 后重启失败，daemon 启动链路必须避免 macOS fork 后继续初始化完整运行时，同时保证 Windows `start --daemon` 可被 upgrade restart 正常调起。

## 实现逻辑

- `install-binary.sh` 下载并安装 CLI 后调用 `run_post_install "$INSTALL_DIR/$binary_name"`。
- Bash installer 下载 release 资产前会对 GitHub 直连和内置镜像源做轻量可用性探测，优先选择最快返回的源；如果被选中的源在完整下载阶段失败，再回退到所有镜像和下载器的竞速下载。
- Bash installer 默认打开下载器可见进度：`curl` 使用 progress bar，`wget` 使用 forced progress bar，`aria2c` 使用 1 秒 summary，`axel` 保持默认进度输出；内部竞速候选通过 `BIFROST_DOWNLOAD_PROGRESS=0` 保持安静，避免并发进度条互相污染。
- PowerShell installer 使用同一组镜像候选源和短超时探测，latest、archive、checksums 都通过选出的最快可用源下载；如果选中源完整下载失败，则继续按候选源列表回退。
- PowerShell installer 在下载开始、结束时使用 `Write-Progress` 明确展示下载状态，避免终端完全无反馈。
- Bash installer、PowerShell installer 和 `bifrost upgrade` 都必须先把新二进制复制到同目录临时文件，完成权限和 macOS xattr 处理后再原子替换最终路径。禁止直接复制到最终 `bifrost` 路径，避免 guardian、自检或 post-install 在复制窗口执行到半成品 Mach-O 并卡在 macOS launched-suspended / dyld 阶段。`bifrost upgrade` 的旧二进制 backup 必须保留到新二进制 `--version` 校验和 musl fallback 全部收敛；任一校验或 fallback 失败时恢复旧二进制，最终成功后再清理 backup。
- `bifrost upgrade` 的手动安装路径使用与安装脚本一致的 GitHub / mirror 候选列表，先并发探测 release 资产 URL，选择最快可用源，再执行带进度百分比、已下载大小和速度的 streaming 下载；若选中源完整下载失败，继续回退剩余候选源。
- `bifrost upgrade` 在替换二进制后若检测到运行中的 daemon，会优先复用 runtime.json 中记录的端口、host、socks5 端口和系统代理状态重启代理；如果 runtime.json 不存在，则按默认配置执行 `start -d -y --skip-cert-check`，端口、host 和 socks5 由当前配置默认值决定。系统代理参数按“当前系统代理快照优先 -> runtime 明确启用/关闭 -> 无 runtime 时读取默认配置 -> legacy runtime 缺失字段时显式 `--no-system-proxy`”构造，避免旧 daemon 原本未启用系统代理时，upgrade restart 因配置默认值回落而意外启动系统代理 helper，也避免无 runtime 文件时丢失用户当前默认配置。重启路径在 `stop_for_restart` 成功后必须等待旧 HTTP 端口和 separate SOCKS5 端口全部释放，再执行 `start -d`；如果任一端口在 10 秒内仍被占用，upgrade 先执行系统代理 crash recovery 并清理 restart shutdown marker，再返回包含占用进程信息的错误，避免新 daemon 因 `EADDRINUSE` 立即退出并被包装成模糊的 readiness/network error，也避免系统代理继续指向不可用的 Bifrost 端口。该端口释放等待在 Unix 与 Windows 上行为一致（共用 `wait_for_port_released` 的 bind 探测，`wait_for_restart_port_release` 以 `cfg(any(unix, windows))` 编译），Windows 下端口仍被占用时给出 `netstat -ano | findstr :<port>` 形式的诊断提示；仅在既非 Unix 也非 Windows 的平台保留 no-op 回退。upgrade restart 使用升级前确定的安装目标路径或 Homebrew 的 `bifrost` PATH 入口启动新进程，禁止在二进制替换完成后再用 `current_exe()` 推断新二进制路径。
- Windows 上当前运行的 `bifrost.exe` 不能被同一进程直接替换；`bifrost upgrade` 检测到手动安装目标就是当前 exe 时必须提前给出明确错误，引导用户使用外部 PowerShell 安装脚本或 updater 进程，不能在 `remove_file` 阶段才暴露模糊的 permission denied。
- macOS 和 Windows 上 `start --daemon` 不再让 fork 出来的 child 继续执行完整代理初始化，Windows 也不再返回 daemon unsupported。父进程改为通过 `std::process::Command` 启动当前二进制的 exec 子进程，并设置内部环境变量 `BIFROST_DETACHED_DAEMON_CHILD=1`。exec 子进程绕过二次 daemon fork，按前台启动路径初始化服务，但继续写入 `runtime_start_mode=daemon`，保留 daemon 的系统代理生命周期语义；父进程重定向 stdout/stderr 到 daemon log，切换 child working directory 到 `BIFROST_DATA_DIR`，macOS/Linux 使用 `setsid()` 脱离当前终端，Windows 使用 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`，并等待代理监听端口 ready 或 child 提前退出。ready 探测和 Admin UI 提示会把 `0.0.0.0`、`::`、`[::]` 归一到 `127.0.0.1`，避免 wildcard listener 被误当成不可连接地址。该链路避免 macOS Objective-C runtime 在 fork 后首次初始化时触发 `objc_initializeAfterForkError`，同时保留后台进程可 stop/restart/status 管理的用户行为。
- `bifrost upgrade` 替换二进制后的 `bifrost --version` 校验必须有硬超时。该校验只用于判断新二进制是否可运行，不能把整个 upgrade 命令无限卡住；超时按校验失败处理并进入既有错误/回退提示路径。
- Debug 构建提供本地 upgrade E2E 专用环境变量 `BIFROST_UPGRADE_TEST_LATEST_VERSION` 和 `BIFROST_UPGRADE_TEST_ARCHIVE`，用于在无网络、无真实 GitHub release 的本机测试中构造“旧 daemon 正在运行 -> `upgrade -y --restart` 下载/解压本地 archive -> 替换安装路径二进制 -> stop 旧 daemon -> 用新二进制 restart”的完整链路。该测试入口只在 `debug_assertions` 下编译，正式 release 不读取这些变量。
- 最新版本探测不再按 `github.com -> mirror` 串行等待完整超时；Bash 通过并发重定向探测抢最快结果，PowerShell 通过短超时探测先选源再读取 `releases/latest` 重定向，避免默认 GitHub 直连在受限网络中拖到完整下载超时。
- `BIFROST_GITHUB_MIRROR` 仍作为优先候选源保留，`BIFROST_DOWNLOAD_CONNECT_TIMEOUT`、`BIFROST_DOWNLOAD_TIMEOUT`、`BIFROST_DOWNLOAD_TRIES` 继续控制下载；`BIFROST_MIRROR_PROBE_TIMEOUT` 控制镜像轻量探测超时，默认 5 秒。Bash installer 与 `bifrost upgrade` 均读取这些环境变量。
- 默认 post-install 顺序固定为：
  1. `bifrost ca install`
  2. `bifrost install-skill --tool all -y`
  3. `bifrost start --daemon --yes`
- 使用安装目录中的绝对二进制路径执行命令，避免当前 shell 的 `PATH` 尚未刷新时找不到 `bifrost`。
- `start --daemon --yes` 保持 `bifrost start` 的默认正式实例语义，同时自动确认启动过程中的证书检查和已有进程重启提示，并让安装脚本能够正常返回。
- post-install 单步失败只记录 warning 和可重试命令，不回滚已经安装好的 CLI 二进制。原因是证书信任可能受系统权限、管理员授权或平台安全策略影响，失败时用户仍应保留可用 CLI。
- post-install 每个子命令默认有 `BIFROST_INSTALL_POST_INSTALL_TIMEOUT=120` 秒 watchdog。证书安装、skills 安装或服务启动任一步骤卡住时，安装脚本返回该步骤失败并继续收敛后续提示，不能永久占住用户终端。
- 提供全局和分步跳过能力：
  - `--no-post-install` / `BIFROST_INSTALL_POST_INSTALL=0`
  - `--no-install-cert` / `BIFROST_INSTALL_AUTO_CERT=0`
  - `--no-install-skills` / `BIFROST_INSTALL_AUTO_SKILLS=0`
  - `--no-start` / `BIFROST_INSTALL_AUTO_START=0`
- `BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1` 仅用于自动化测试，打印将执行的命令而不真正修改系统证书、skills 或代理进程。

## 依赖项

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `install-binary.sh`
- `install-binary.ps1`
- `crates/bifrost-cli/src/commands/upgrade.rs`
- `crates/bifrost-cli/src/commands/ca.rs`
- `crates/bifrost-cli/src/commands/install_skill.rs`
- `crates/bifrost-cli/src/commands/start.rs`
- `crates/bifrost-cli/src/main.rs`
- `e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `e2e-tests/tests/test_install_binary_post_install.sh`
- `e2e-tests/tests/test_upgrade_local_restart_e2e.sh`
- `human_tests/install-binary-one-click.md`
- `human_tests/cli-start-stop-status.md`

## 测试方案

### 单元测试

- `upgrade_download_progress_formats_percent_and_size`：验证 `bifrost upgrade` 进度行包含百分比、已下载大小、总大小和速度。
- `upgrade_github_path_url_joins_mirror_and_release_path`：验证镜像 base 与 GitHub release path 拼接正确。
- `upgrade_mirror_display_name_hides_full_path`：验证终端展示隐藏镜像 URL 后半段，避免过长路径污染输出。
- `upgrade_download_tuning_parses_positive_values` / `upgrade_download_tuning_rejects_invalid_values`：验证 upgrade 下载超时、探测超时和重试次数解析与安装脚本一致。
- `upgrade_install_binary_atomically_replaces_existing_target`：验证 upgrade 先写临时文件再替换目标二进制，替换后临时文件不存在且旧二进制 backup 保留到最终校验阶段。
- `upgrade_restore_binary_backup_restores_previous_target`：验证新二进制校验失败或 fallback 失败时可把旧二进制从 backup 恢复到目标路径。
- `upgrade_command_status_with_timeout_reports_success_and_failure`：验证升级子进程超时 helper 能区分成功退出和非 0 退出。
- `upgrade_command_status_with_timeout_does_not_block_on_hung_child`：验证升级子进程超时 helper 遇到长时间不退出的命令时快速返回 `TimedOut`，不等待子进程完整结束。
- `wait_for_port_released_returns_quickly_when_port_is_free` / `wait_for_port_released_times_out_when_port_is_held`：验证 upgrade/restart 共用的端口释放等待工具在空闲端口快速返回、占用端口耗尽预算。
- `upgrade_restart_port_from_runtime_defaults_to_9900` / `upgrade_restart_ports_from_runtime_uses_runtime_ports`：验证 upgrade restart 在 legacy pidfile 和 runtime.json 场景选择正确的等待端口，并覆盖 separate SOCKS5 端口。
- `env_flag_enabled_accepts_true_values` / `env_flag_enabled_rejects_absent_and_false_values`：验证内部 exec 子进程标记只接受明确 true 值，避免普通命令被误判成 detached daemon child。
- `test_build_restart_args_no_runtime_info_uses_default_config_system_proxy` / `test_build_restart_args_no_runtime_info_preserves_disabled_default_config_system_proxy`：验证 runtime.json 缺失时 upgrade restart 使用默认配置构造系统代理参数。
- `detached_daemon_readiness_host_maps_wildcard_listeners_to_loopback`：验证 `0.0.0.0`、`::`、`[::]` ready 探测地址归一到 loopback。
- 使用 `bash -n install-binary.sh` 覆盖 shell 语法。

### E2E 测试

- 新增 `e2e-tests/tests/test_install_binary_adaptive_download.sh`：
  - source `install-binary.sh` 并设置 `BIFROST_INSTALL_BINARY_SKIP_MAIN=1`，避免真实下载 release。
  - stub `probe_github_url`，验证默认 GitHub 不可用时会选择 `https://ghfast.top/https://github.com`。
  - stub `get_latest_version_via_redirect`，验证最新版本探测使用并发最快镜像结果。
  - stub `download_file`，验证完整下载优先使用已探测出的最快源。
  - stub `download_github_file_race`，验证最快源完整下载失败后仍回退到旧的全镜像竞速路径。
  - stub `curl`，验证默认下载不再使用静默 `-s`，而是启用 `--progress-bar`。
  - stub `download_with_tool`，验证并发竞速候选下载设置 `BIFROST_DOWNLOAD_PROGRESS=0`，避免多进度条混杂。
  - 验证 help 暴露 `BIFROST_MIRROR_PROBE_TIMEOUT`。
- 新增 `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`：
  - 设置 `BIFROST_INSTALL_BINARY_SKIP_MAIN=1` 后 dot-source `install-binary.ps1`，避免真实安装。
  - stub `Test-GithubUrl`，验证默认 GitHub 不可用时会选择 `https://ghfast.top/https://github.com`。
  - stub `Get-LatestVersionViaRedirect`，验证 latest 版本探测使用选中的镜像结果。
  - stub `Invoke-BifrostDownload`，验证 archive 下载优先使用已探测出的最快源。
  - 验证最快源完整下载失败后继续回退到 `github.com`。
  - 验证 `BIFROST_DOWNLOAD_TIMEOUT` 和 `BIFROST_DOWNLOAD_TRIES` 在 PowerShell installer 中可解析。
- 新增 `e2e-tests/tests/test_install_binary_post_install.sh`：
  - source `install-binary.sh` 并设置 `BIFROST_INSTALL_BINARY_SKIP_MAIN=1`，避免真实下载 release。
  - 设置 `BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1`，验证默认命令顺序为 `ca install` -> `install-skill --tool all -y` -> `start --daemon --yes`。
  - 验证 `BIFROST_INSTALL_POST_INSTALL=0` 不执行任何 post-install 命令。
  - 验证 `BIFROST_INSTALL_AUTO_CERT=0`、`BIFROST_INSTALL_AUTO_SKILLS=0`、`BIFROST_INSTALL_AUTO_START=0` 可分别跳过证书、skills、启动。
  - 验证 Bash installer 的 `install_binary_atomically` 先写临时文件再替换目标路径，替换后临时文件不存在。
  - 验证 `BIFROST_INSTALL_POST_INSTALL_TIMEOUT=1` 时，卡住的 post-install 子命令返回 124，安装脚本不无限等待。
  - 验证 `--help` 展示 post-install opt-out 参数和环境变量。
- 更新 `e2e-tests/tests/test_upgrade_restart_e2e.sh`：
  - 保留无 daemon、有 daemon 但已最新、`--restart` 已最新和 runtime.json 参数回归。
  - 增加源码门禁，确认 upgrade 的真实重启路径包含 `wait_for_restart_ports_release`、HTTP/SOCKS5 多端口收集、端口占用错误文案、`find_process_on_port` 诊断和系统代理恢复，避免后续改动绕过端口释放保护或失败恢复。
  - 增加 Windows 覆盖门禁，确认 `wait_for_restart_port_release` 与 `wait_for_port_released` 以 `cfg(any(unix, windows))` 编译、不残留 `cfg(not(unix))` 的 unix-only 回退，避免 Windows upgrade 重启再次退回到不等端口释放的竞态路径。
  - 增加 macOS/Windows daemon exec child 源码门禁，确认 `start --daemon` 使用 exec 子进程、`BIFROST_DETACHED_DAEMON_CHILD`、`setsid()`、Windows detached process flags、`current_dir(&bifrost_dir)`、wildcard host ready 探测归一和 main daemon bypass，防止重回 fork 后初始化完整运行时的崩溃路径，并避免 Windows upgrade restart 无法启动后台服务。
  - 增加 daemon stop 后 helper 清理断言，确认当前测试数据目录下不残留 `bifrost __tray` 进程，避免用户升级/重启后服务已停但 helper 仍残留。
- 新增 `e2e-tests/tests/test_upgrade_local_restart_e2e.sh`：
  - 选择本机旧版 `0.0.99` 二进制作为运行中的旧 daemon；通过 `BIFROST_BIN` 指向当前源码构建出的新二进制。
  - 创建临时安装目录和本地 release archive，使用 debug-only upgrade hook 把 archive 作为最新版本下载源。
  - 执行真实 `upgrade -y --restart`，断言输出包含检测运行中代理、停止旧代理、等待端口释放、重启成功四个里程碑。
  - 断言 restart 后 PID 变化、新 daemon 使用升级后的安装路径、Admin API ready、错误日志没有 ObjC fork-safety crash、upgrade 输出与新 runtime 都保留 `--no-system-proxy` 模式、stop 后端口释放且无同数据目录 tray helper 残留。

### 真实场景测试

- 新增 `human_tests/install-binary-one-click.md`：
  - 默认镜像自适应用例：通过 stub 网络探测函数模拟 GitHub 直连不可用，验证安装脚本选择更快镜像。
  - Windows 镜像自适应用例：通过 PowerShell installer 测试脚本验证 `.ps1` latest、archive、fallback 和 timeout env 行为。
  - 下载回退用例：通过 stub 完整下载失败，验证脚本仍保留旧的全镜像竞速兜底。
  - 临时目录真实安装用例：设置 `BIFROST_INSTALL_DIR=$(mktemp -d)`、`--no-post-install --no-modify-path`，验证 latest 探测、release 下载、checksum 校验、解压和 `bifrost --version` 全链路通过且不修改系统状态。
  - 默认 dry-run 输出包含证书安装、全量 skill 安装和服务启动命令。
  - 验证命令顺序符合一键体验目标。
  - 验证全局 opt-out 和分步 opt-out。
  - 验证 help 文案可发现。
  - 验证 Bash installer 默认下载进度可见，竞速候选进度被抑制。
  - 验证 installer 与 `bifrost upgrade` 不会在最终可执行路径暴露半成品二进制，避免并发自检执行到未完成 Mach-O。
  - 验证 `bifrost upgrade` 的最快源选择、进度百分比和 env 超时/重试解析。
  - 验证 `bifrost upgrade` 的二进制校验子进程有硬超时，`bifrost --version` 卡住不会拖死整个升级流程。
  - 验证 `bifrost upgrade` 重启路径在 stop 后等待端口释放，端口仍被占用时先恢复系统代理、再输出明确诊断，不再把 `EADDRINUSE` 包装成模糊 readiness/network error。
  - 验证 runtime.json 缺失时 upgrade restart 等同于使用当前默认配置启动，仍保留默认配置中的 system proxy 开关和 bypass；legacy runtime 缺失 system proxy 字段时继续显式关闭 system proxy。
  - 验证 macOS/Windows daemon exec child 源码门禁、Windows detached process flags 和 wildcard listener ready 探测归一。
  - 验证本地构造的真实 upgrade restart 链路：旧 daemon 运行中，`upgrade -y --restart` 替换为本地 archive 中的新二进制，新 daemon 自动启动并可停止。
- 更新 `human_tests/cli-start-stop-status.md`：
  - 新增 macOS daemon exec child 回归用例，使用临时数据目录启动真实 daemon，确认 start/status/stop 均正常、`runtime.json` 仍标记 daemon、日志中不再出现 `+[NSNumber initialize]` 或 `objc_initializeAfterForkError`，stop 后不残留同数据目录的 tray helper。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：安装脚本下载进度、`bifrost upgrade` 下载进度、upgrade 最快源并发选择、upgrade restart 端口释放等待、macOS daemon fork/ObjC 崩溃规避、既有 post-install 行为是否全部覆盖。
- 复核变更范围：`git status --short`、`git diff`，确认未触碰既有 im-gateway 改动。
- 代码 review：检查 `install-binary.sh` 下载器进度参数、竞速候选安静模式、`PATH` 未刷新、权限失败、CI opt-out、dry-run 下的行为。
- 代码 review：检查 upgrade 镜像探测不会破坏用户指定 `BIFROST_GITHUB_MIRROR`，被选中源失败后仍能回退旧下载路径；检查 PowerShell env 变量、latest、archive、checksum 下载路径与 Bash installer 保持一致；检查 upgrade restart 不会在端口仍被占用时启动新 daemon，且失败时会恢复系统代理；检查 macOS daemon exec child 不触发二次 daemon fork、不丢失 data dir、setsid、runtime.json daemon 标记、系统代理生命周期和 stop 能力。
- 复测命令：`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib env_flag_enabled`、`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`bash e2e-tests/tests/test_upgrade_restart_e2e.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）。

### 第 2 轮

- 再次对照第 1 轮 diff 和测试输出，检查文档、E2E、human_tests/readme、Cargo.lock 是否同步。
- 复测命令：`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib env_flag_enabled`、`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`bash e2e-tests/tests/test_upgrade_restart_e2e.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）、human_tests 中列出的真实 daemon 命令。

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
- `bash e2e-tests/tests/test_install_binary_post_install.sh`
- `bash e2e-tests/tests/test_upgrade_restart_e2e.sh`
- `bash e2e-tests/tests/test_install_musl_fallback.sh`
- `cargo fmt --all -- --check`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `scripts/ci/local-ci.sh`：脚本和文档变更可按成本评估执行；未执行需在交付中说明。

## 文档更新要求

- 更新 `README.md` 和 `docs/getting-started.md`，说明一键安装默认会完成证书、skills 和后台服务启动。
- 同步更新站点安装页 `site/src/content/docs/getting-started/installation.mdx`。
- 更新 `human_tests/readme.md` 索引。
