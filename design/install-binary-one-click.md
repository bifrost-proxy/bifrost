# 二进制安装脚本一键体验

## 功能模块说明

`install-binary.sh` 是用户通过 `curl ... | bash` 使用的官方远程二进制安装入口。安装完成后，脚本默认继续完成一键体验初始化：

- 安装并信任 Bifrost CA 证书。
- 安装所有支持 AI 工具的 Bifrost skills。
- 启动 Bifrost 服务，用户安装完成后可直接访问默认管理端和代理能力。

目标是把原先“安装二进制后再手动处理证书、skill、启动服务”的多步流程合并为一次安装命令。高级用户和 CI 仍可通过参数或环境变量跳过自动步骤。本轮同时修复二进制下载长时间静默的问题，并让 `bifrost upgrade` 复用安装脚本的最快源选择能力。

## 实现逻辑

- `install-binary.sh` 下载并安装 CLI 后调用 `run_post_install "$INSTALL_DIR/$binary_name"`。
- Bash installer 下载 release 资产前会对 GitHub 直连和内置镜像源做轻量可用性探测，优先选择最快返回的源；如果被选中的源在完整下载阶段失败，再回退到所有镜像和下载器的竞速下载。
- Bash installer 默认打开下载器可见进度：`curl` 使用 progress bar，`wget` 使用 forced progress bar，`aria2c` 使用 1 秒 summary，`axel` 保持默认进度输出；内部竞速候选通过 `BIFROST_DOWNLOAD_PROGRESS=0` 保持安静，避免并发进度条互相污染。
- PowerShell installer 使用同一组镜像候选源和短超时探测，latest、archive、checksums 都通过选出的最快可用源下载；如果选中源完整下载失败，则继续按候选源列表回退。
- PowerShell installer 在下载开始、结束时使用 `Write-Progress` 明确展示下载状态，避免终端完全无反馈。
- `bifrost upgrade` 的手动安装路径使用与安装脚本一致的 GitHub / mirror 候选列表，先并发探测 release 资产 URL，选择最快可用源，再执行带进度百分比、已下载大小和速度的 streaming 下载；若选中源完整下载失败，继续回退剩余候选源。
- `bifrost upgrade` 在替换二进制后若检测到运行中的 daemon，会复用 runtime.json 中记录的端口、host、socks5 端口和系统代理快照重启代理。重启路径在 `stop_for_restart` 成功后必须等待旧监听端口完全释放，再执行 `start -d`；如果端口在 10 秒内仍被占用，upgrade 直接返回包含占用进程信息的错误，避免新 daemon 因 `EADDRINUSE` 立即退出并被包装成模糊的 readiness/network error。
- 最新版本探测不再按 `github.com -> mirror` 串行等待完整超时；Bash 通过并发重定向探测抢最快结果，PowerShell 通过短超时探测先选源再读取 `releases/latest` 重定向，避免默认 GitHub 直连在受限网络中拖到完整下载超时。
- `BIFROST_GITHUB_MIRROR` 仍作为优先候选源保留，`BIFROST_DOWNLOAD_CONNECT_TIMEOUT`、`BIFROST_DOWNLOAD_TIMEOUT`、`BIFROST_DOWNLOAD_TRIES` 继续控制下载；`BIFROST_MIRROR_PROBE_TIMEOUT` 控制镜像轻量探测超时，默认 5 秒。Bash installer 与 `bifrost upgrade` 均读取这些环境变量。
- 默认 post-install 顺序固定为：
  1. `bifrost ca install`
  2. `bifrost install-skill --tool all -y`
  3. `bifrost start --daemon --yes`
- 使用安装目录中的绝对二进制路径执行命令，避免当前 shell 的 `PATH` 尚未刷新时找不到 `bifrost`。
- `start --daemon --yes` 保持 `bifrost start` 的默认正式实例语义，同时自动确认启动过程中的证书检查和已有进程重启提示，并让安装脚本能够正常返回。
- post-install 单步失败只记录 warning 和可重试命令，不回滚已经安装好的 CLI 二进制。原因是证书信任可能受系统权限、管理员授权或平台安全策略影响，失败时用户仍应保留可用 CLI。
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
- `e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `e2e-tests/tests/test_install_binary_post_install.sh`
- `human_tests/install-binary-one-click.md`

## 测试方案

### 单元测试

- `upgrade_download_progress_formats_percent_and_size`：验证 `bifrost upgrade` 进度行包含百分比、已下载大小、总大小和速度。
- `upgrade_github_path_url_joins_mirror_and_release_path`：验证镜像 base 与 GitHub release path 拼接正确。
- `upgrade_mirror_display_name_hides_full_path`：验证终端展示隐藏镜像 URL 后半段，避免过长路径污染输出。
- `upgrade_download_tuning_parses_positive_values` / `upgrade_download_tuning_rejects_invalid_values`：验证 upgrade 下载超时、探测超时和重试次数解析与安装脚本一致。
- `wait_for_port_released_returns_quickly_when_port_is_free` / `wait_for_port_released_times_out_when_port_is_held`：验证 upgrade/restart 共用的端口释放等待工具在空闲端口快速返回、占用端口耗尽预算。
- `upgrade_restart_port_from_runtime_defaults_to_9900` / `upgrade_restart_port_from_runtime_uses_runtime_port`：验证 upgrade restart 在 legacy pidfile 和 runtime.json 场景选择正确的等待端口。
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
  - 验证 `--help` 展示 post-install opt-out 参数和环境变量。
- 更新 `e2e-tests/tests/test_upgrade_restart_e2e.sh`：
  - 保留无 daemon、有 daemon 但已最新、`--restart` 已最新和 runtime.json 参数回归。
  - 增加源码门禁，确认 upgrade 的真实重启路径包含 `wait_for_restart_port_release`、端口占用错误文案和 `find_process_on_port` 诊断，避免后续改动绕过端口释放保护。

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
  - 验证 `bifrost upgrade` 的最快源选择、进度百分比和 env 超时/重试解析。
  - 验证 `bifrost upgrade` 重启路径在 stop 后等待端口释放，端口仍被占用时输出明确诊断，不再把 `EADDRINUSE` 包装成模糊 readiness/network error。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：安装脚本下载进度、`bifrost upgrade` 下载进度、upgrade 最快源并发选择、upgrade restart 端口释放等待、既有 post-install 行为是否全部覆盖。
- 复核变更范围：`git status --short`、`git diff`，确认未触碰既有 im-gateway 改动。
- 代码 review：检查 `install-binary.sh` 下载器进度参数、竞速候选安静模式、`PATH` 未刷新、权限失败、CI opt-out、dry-run 下的行为。
- 代码 review：检查 upgrade 镜像探测不会破坏用户指定 `BIFROST_GITHUB_MIRROR`，被选中源失败后仍能回退旧下载路径；检查 PowerShell env 变量、latest、archive、checksum 下载路径与 Bash installer 保持一致；检查 upgrade restart 不会在端口仍被占用时启动新 daemon。
- 复测命令：`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`、`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`bash e2e-tests/tests/test_upgrade_restart_e2e.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）。

### 第 2 轮

- 再次对照第 1 轮 diff 和测试输出，检查文档、E2E、human_tests/readme、Cargo.lock 是否同步。
- 复测命令：`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`、`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`bash e2e-tests/tests/test_upgrade_restart_e2e.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）、human_tests 中列出的 dry-run 命令。

## 校验要求

- `grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml`
- `grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml`
- `grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml`
- `bash -n install-binary.sh`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`
- `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`
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
