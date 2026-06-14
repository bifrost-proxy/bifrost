# 二进制安装脚本一键体验真实场景测试

## 功能模块说明

验证 `install-binary.sh`、`install-binary.ps1` 和 `bifrost upgrade` 在远程二进制安装/升级时会自动探测更快的 GitHub/mirror 下载源，并在下载阶段展示用户可感知进度；同时验证安装完成后默认规划并执行证书安装/信任、全量 skill 安装和 Bifrost 服务启动，形成一键安装、一键体验流程。为避免真实测试修改系统证书、skills 目录或系统代理，本用例使用脚本内置 dry-run post-install 路径和离线网络 stub 验证用户可感知命令编排。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 除 TC-IBOC-08 外，不下载 release；所有用例都不启动真实 Bifrost 服务，不修改系统代理。
- 所有用例都不安装真实 CA 证书，不写入真实 AI tool skills 目录。
- 下载源自适应用例通过 shell stub 模拟网络探测和下载结果，不访问真实 GitHub 或镜像。
- 下载进度用例通过 shell/Rust 单元测试验证终端输出参数与进度格式，不依赖真实大文件下载。
- Windows installer 用例需要 `pwsh` 或 Windows PowerShell；如果当前机器不可用，必须记录为环境阻塞，不能宣称已执行通过。
- CI Cargo 网络稳定性用例通过读取 GitHub Actions workflow，验证 CI/Release 统一关闭 Cargo HTTP/2 multiplexing、开启网络重试并提高 HTTP timeout。
- 所有命令执行前使用：
  ```bash
  source ~/.zshrc
  ```

## 测试用例列表

### TC-IBOC-01 默认 post-install 包含证书、skills 和服务启动

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 run_post_install /tmp/bifrost-test-bin
   ```
2. 检查输出包含：
   ```text
   [dry-run] /tmp/bifrost-test-bin ca install
   [dry-run] /tmp/bifrost-test-bin install-skill --tool all -y
   [dry-run] /tmp/bifrost-test-bin start --daemon --yes
   ```

预期结果：

- 默认一键流程会安装并信任 CA 证书。
- 默认一键流程会安装所有支持 AI 工具的 Bifrost skills。
- 默认一键流程会启动 Bifrost 服务，并通过 `--yes` 自动确认启动过程中的交互提示。

### TC-IBOC-02 默认 post-install 命令顺序正确

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   bash e2e-tests/tests/test_install_binary_post_install.sh
   ```
2. 观察顺序断言结果。

预期结果：

- `ca install` 先于 `install-skill --tool all -y`。
- `install-skill --tool all -y` 先于 `start --daemon --yes`。
- E2E 脚本输出所有断言 PASS。

### TC-IBOC-03 全局 opt-out 可跳过 post-install

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 BIFROST_INSTALL_POST_INSTALL=0 run_post_install /tmp/bifrost-test-bin
   ```
2. 检查输出包含：
   ```text
   Post-install setup skipped
   ```
3. 检查输出不包含：
   ```text
   [dry-run] /tmp/bifrost-test-bin ca install
   [dry-run] /tmp/bifrost-test-bin install-skill --tool all -y
   [dry-run] /tmp/bifrost-test-bin start --daemon --yes
   ```

预期结果：

- CI 或高级用户可以一次性跳过证书、skills 和自动启动。

### TC-IBOC-04 分步 opt-out 可分别跳过证书、skills 和启动

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 \
   BIFROST_INSTALL_AUTO_CERT=0 \
   BIFROST_INSTALL_AUTO_SKILLS=0 \
   BIFROST_INSTALL_AUTO_START=0 \
     run_post_install /tmp/bifrost-test-bin
   ```
2. 检查输出包含：
   ```text
   CA certificate installation skipped
   Bifrost skill installation skipped
   Bifrost service startup skipped
   ```

预期结果：

- 用户可以分别跳过证书安装、skills 安装或自动启动，不影响其他安装脚本逻辑。

### TC-IBOC-05 help 文案暴露一键体验开关

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   bash ./install-binary.sh --help
   ```
2. 检查输出包含：
   ```text
   --no-post-install
   --no-install-cert
   --no-install-skills
   --no-start
   BIFROST_INSTALL_AUTO_START
   ```

预期结果：

- 用户可从 help 中发现默认 post-install 行为的跳过方式。

### TC-IBOC-06 GitHub 直连不可用时自动选择更快镜像

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   bash e2e-tests/tests/test_install_binary_adaptive_download.sh
   ```
2. 检查输出包含：
   ```text
   PASS fastest mirror probe selection
   PASS latest version redirect race
   PASS selected source full download
   ```

预期结果：

- 当 stub 模拟 `github.com` 探测失败且 `ghfast.top` 探测成功时，安装脚本选择 `https://ghfast.top/https://github.com`。
- 最新版本探测使用最快镜像的 `releases/latest` 重定向结果，不再等待默认 GitHub 直连串行超时。
- 完整 release 资产下载优先使用探测出的最快镜像 URL。

### TC-IBOC-07 最快源完整下载失败后回退全镜像竞速

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   bash e2e-tests/tests/test_install_binary_adaptive_download.sh
   ```
2. 检查输出包含：
   ```text
   PASS fallback full mirror race
   PASS help documents mirror probe timeout
   ```

预期结果：

- 当 stub 模拟最快源在完整下载阶段失败时，安装脚本会继续调用全镜像竞速兜底路径。
- `bash ./install-binary.sh --help` 暴露 `BIFROST_MIRROR_PROBE_TIMEOUT`，用户可调整镜像探测超时。

### TC-IBOC-08 临时目录真实安装链路不修改系统状态

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   TMP_INSTALL_DIR=$(mktemp -d)
   BIFROST_GITHUB_MIRROR='https://ghfast.top/https://github.com' \
   BIFROST_DOWNLOAD_TIMEOUT=45 \
   BIFROST_DOWNLOAD_TRIES=1 \
   BIFROST_INSTALL_DIR="$TMP_INSTALL_DIR" \
     bash install-binary.sh --no-post-install --no-modify-path
   "$TMP_INSTALL_DIR/bifrost" --version
   rm -rf "$TMP_INSTALL_DIR"
   ```
2. 检查输出包含：
   ```text
   Fetching latest version
   Selected fastest available source
   Checksum verified
   CLI installed
   Post-install setup skipped
   bifrost 0.0
   ```

预期结果：

- latest 版本探测、release archive 下载、checksum 下载、校验、解压和二进制运行完整通过。
- 安装目录为临时目录，`--no-post-install --no-modify-path` 不修改系统证书、skills、服务进程、系统代理或 shell PATH。

### TC-IBOC-09 Windows PowerShell installer 下载源自适应

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1
   ```
2. 检查输出包含：
   ```text
   PASS fastest mirror probe selection
   PASS latest version redirect selection
   PASS selected source full download
   PASS fallback full mirror list
   PASS download timeout env
   ```

预期结果：

- `install-binary.ps1` 保留 `BIFROST_GITHUB_MIRROR` 作为优先候选源且不重复。
- 当 stub 模拟 `github.com` 探测失败且 `ghfast.top` 探测成功时，PowerShell installer 选择 `https://ghfast.top/https://github.com`。
- latest 版本探测、完整 archive 下载和 checksums 下载都可基于选出的镜像 URL 构造。
- 当选中源完整下载失败时，PowerShell installer 会继续回退到候选源列表中的 `github.com`。
- `BIFROST_DOWNLOAD_TIMEOUT` 和 `BIFROST_DOWNLOAD_TRIES` 在 PowerShell installer 中可被解析。

### TC-IBOC-10 安装脚本默认显示下载进度且竞速候选保持安静

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   bash e2e-tests/tests/test_install_binary_adaptive_download.sh
   ```
2. 检查输出包含：
   ```text
   PASS curl progress is visible by default
   PASS race candidate downloads keep progress quiet
   ```

预期结果：

- `download_file` 默认调用 `curl` 时使用 `--progress-bar`，不再使用纯静默 `-s` 下载。
- 全镜像竞速候选设置 `BIFROST_DOWNLOAD_PROGRESS=0`，避免多个并发下载器在同一终端输出互相覆盖。
- 用户真实安装时至少能看到下载器提供的百分比、进度条或传输状态，不再长时间无反馈。

### TC-IBOC-11 bifrost upgrade 复用最快源选择并显示下载进度

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli upgrade_ --lib
   ```
2. 检查输出包含：
   ```text
   upgrade_download_progress_formats_percent_and_size ... ok
   upgrade_github_path_url_joins_mirror_and_release_path ... ok
   upgrade_mirror_display_name_hides_full_path ... ok
   upgrade_download_tuning_parses_positive_values ... ok
   upgrade_download_tuning_rejects_invalid_values ... ok
   ```

预期结果：

- `bifrost upgrade` 的 release 下载路径可基于 `github.com` 或镜像 base 构造，支持 `BIFROST_GITHUB_MIRROR` 优先。
- 下载进度行包含百分比、已下载/总大小和速度，例如 `Downloading…  50.0% (512 B/1.0 KiB, .../s)`。
- `BIFROST_DOWNLOAD_CONNECT_TIMEOUT`、`BIFROST_DOWNLOAD_TIMEOUT`、`BIFROST_MIRROR_PROBE_TIMEOUT`、`BIFROST_DOWNLOAD_TRIES` 的正整数解析有效，非法值回退默认值。

### TC-IBOC-12 CI Cargo 依赖下载 HTTP/2 抖动回归

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml
   grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml
   grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml
   grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/release.yml
   grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/release.yml
   grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/release.yml
   ```
2. 检查所有命令退出码为 0。

预期结果：

- CI 和 Release workflow 都为 Cargo 网络层开启 10 次重试、关闭 HTTP/2 multiplexing，并将 HTTP timeout 设置为 120 秒。
- GitHub Actions macOS CLI build 遇到 crates.io sparse index HTTP/2 framing 抖动时不再因一次 `curl failed [16]` 直接失败。

### TC-IBOC-13 Unix/macOS 安装脚本优先使用更小的 tar.xz 发布包

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   get_archive_ext_candidates darwin
   ```
2. 在支持 xz 的环境下，检查输出第一行为 `tar.xz`，第二行为 `tar.gz`。
3. 准备本地 mock release 目录，同时放置 `bifrost-vTEST-x86_64-apple-darwin.tar.xz` 与 `.tar.gz`，stub `download_github_file` 后调用 `install_binary_for_target x86_64-apple-darwin vTEST darwin <tmp-install> <tmp-work>`。
4. 检查安装后的 `<tmp-install>/bifrost --version` 可执行，且实际下载文件优先命中 `.tar.xz`。
5. 将 stub 调整为 `.tar.xz` 下载失败、`.tar.gz` 下载成功，再次调用安装函数。

预期结果：

- Unix/macOS 用户默认下载更小的 `.tar.xz` 包。
- 当本机 `tar` 不支持 xz、或 `.tar.xz` 资产不存在/下载失败时，安装脚本自动回退 `.tar.gz`。
- Windows PowerShell installer 仍使用 `.zip`，不受本用例影响。

### TC-IBOC-14 Release profile 与 Unix/macOS 发布包体积优化配置检查

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   grep -q '^\[profile.release\]' Cargo.toml
   grep -q '^strip = "symbols"' Cargo.toml
   grep -q 'tar -cJvf "\${ARCHIVE_NAME}.tar.xz"' .github/workflows/release.yml
   grep -q 'dist/\*.tar.xz' .github/workflows/release.yml
   grep -q 'get_archive_ext_candidates' install-binary.sh
   grep -q 'release_archive_ext_candidates' crates/bifrost-cli/src/commands/upgrade.rs
   grep -q 'archive.type === "tar.gz" || archive.type === "tar.xz"' scripts/npm-publish.mjs
   ```
2. 检查所有命令退出码为 0。

预期结果：

- 本地和 CI release 构建默认去除符号，降低打包输入体积。
- Release workflow 额外产出 Unix/macOS `.tar.xz` 包，并上传对应 checksum。
- Bash installer 使用 archive candidates 机制优先选择 `.tar.xz`，保留 `.tar.gz` 兼容回退。
- 内置 `bifrost upgrade` 使用同类 archive candidates 机制，避免仍只下载 `.tar.gz`。
- npm publish 脚本仍优先兼容 `.tar.gz`，但可以从 `.tar.xz` artifact 提取平台二进制，避免 npm 渠道被包类型扩展打断。

### TC-IBOC-15 发布前必须覆盖脚本、NPM、Homebrew 与内置 upgrade 的包类型兼容

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   grep -q 'Missing compatible tar.gz for Homebrew/npm/legacy installers' .github/workflows/release.yml
   grep -q 'Missing smaller tar.xz for install script and bifrost upgrade' .github/workflows/release.yml
   grep -q 'Missing npm-compatible Windows zip' .github/workflows/release.yml
   grep -q 'x86_64-apple-darwin.tar.gz' .github/workflows/release.yml
   grep -q 'aarch64-apple-darwin.tar.gz' .github/workflows/release.yml
   ```
2. 检查所有命令退出码为 0。

预期结果：

- Release job 在 npm publish 前校验所有 CLI artifact 目录存在。
- Unix/macOS artifact 必须同时包含 `.tar.gz` 与 `.tar.xz`：`.tar.gz` 继续服务 Homebrew、npm、旧脚本和人工下载，`.tar.xz` 服务更小下载路径。
- Windows artifact 必须包含 `.zip`，PowerShell installer、npm Windows 平台包和 Windows upgrade 不受 Unix 包类型变化影响。
- Homebrew formula 仍读取 macOS `.tar.gz` checksum，不被新增 `.tar.xz` 影响。

### TC-IBOC-16 优化下载路径失败时必须回退到线上稳定包类型

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh
   BIFROST_DISABLE_XZ_ARCHIVE=1 get_archive_ext_candidates darwin
   ```
2. 检查输出只包含 `tar.gz`，证明 Bash installer 可通过环境变量禁用 `.tar.xz` 并回到旧稳定包。
3. 执行：
   ```bash
   source ~/.zshrc
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli upgrade_archive_candidates_prefer_xz_then_keep_gz_compatibility --lib -- --nocapture
   ```
4. 准备本地 HTTP mock 旧 release，按 `bifrost-proxy/bifrost/releases/download/vOLD/` 目录只放 `.tar.gz` 与旧 checksum，不放 `.tar.xz`；将 `BIFROST_GITHUB_MIRROR` 与 `DEFAULT_GITHUB_MIRROR_URLS` 收窄到本地源后真实调用 `install_binary_for_target x86_64-apple-darwin vOLD darwin <tmp-install> <tmp-work>`。
5. 执行 `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`，检查可选 `.tar.xz` 探测失败时不会进入全镜像竞速。
6. 对线上旧 release 执行临时目录安装：
   ```bash
   source ~/.zshrc
   tmp_install=$(mktemp -d)
   BIFROST_INSTALL_DIR="$tmp_install" \
   BIFROST_INSTALL_POST_INSTALL=0 \
   BIFROST_INSTALL_AUTO_CERT=0 \
   BIFROST_INSTALL_AUTO_SKILLS=0 \
   BIFROST_INSTALL_AUTO_START=0 \
   bash ./install-binary.sh --version v0.0.96 --no-post-install --no-modify-path
   "$tmp_install/bifrost" --version
   rm -rf "$tmp_install"
   ```
7. 准备本地 mock npm artifact，分别只放 `.tar.xz` 和同时放 `.tar.gz/.tar.xz`，执行 Node tar 提取校验。

预期结果：

- Bash installer 在 xz 被禁用或不可用时回退 `.tar.gz`。
- main 分支新安装脚本在新 release 发布前面对旧 latest release 时，即使 `.tar.xz` 不存在，也会快速回退已有 `.tar.gz`，显示 `.tar.gz` 下载进度，并按旧 checksum 校验。
- `.tar.xz` 下载失败后，全镜像下载竞速必须能结束并继续下一候选包；不得因后台子进程 wait 范围过大而卡住。
- 可选 `.tar.xz` 探测不到资产时不得进入静默全镜像竞速，避免用户长时间看不到下载进度。
- 内置 `bifrost upgrade` 候选顺序为 `.tar.xz -> .tar.gz`，禁用 xz 时只使用 `.tar.gz`，Windows 只使用 `.zip`。
- npm publish 仍优先使用 `.tar.gz` 生成平台包，同时能从 `.tar.xz` artifact 兜底提取二进制。
- 任何新增下载优化失败都不会删除或绕过 `.tar.gz` / `.zip` 这条线上稳定路径。

### TC-IBOC-17 bifrost upgrade 重启前必须等待旧端口释放

操作步骤：

1. 执行：
   ```bash
   source ~/.zshrc
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released
   bash e2e-tests/tests/test_upgrade_restart_e2e.sh
   ```
2. 检查 `cargo test -p bifrost-cli --lib upgrade_` 输出包含：
   ```text
   upgrade_restart_port_from_runtime_defaults_to_9900 ... ok
   upgrade_restart_port_from_runtime_uses_runtime_port ... ok
   ```
3. 检查 `cargo test -p bifrost-cli --lib wait_for_port_released` 输出包含：
   ```text
   wait_for_port_released_returns_quickly_when_port_is_free ... ok
   wait_for_port_released_times_out_when_port_is_held ... ok
   ```
4. 检查 `test_upgrade_restart_e2e.sh` 输出包含：
   ```text
   PASS upgrade restart has port-release guard, listener diagnostics, and system proxy recovery
   PASS upgrade restart port-release guard and wait helper cover Windows
   ```

预期结果：

- `bifrost upgrade` 检测到运行中的 daemon 并完成二进制替换后，先执行 `stop_for_restart`，再等待 runtime 端口释放，最后才执行 `start -d`。
- 如果端口在等待窗口内仍被占用，upgrade 返回包含 `Proxy port ... still occupied` 和当前监听进程信息的错误；不得继续启动一个注定因 `Address already in use` 退出的新 daemon。
- 如果旧 daemon 已停止但替代 daemon 因端口未释放而不会启动，upgrade 必须先执行系统代理 crash recovery，再清理 restart shutdown marker，避免用户系统代理继续指向不可用的 Bifrost 端口。
- 端口释放等待必须同时覆盖 Unix 和 Windows；Windows 不得回落到 stop 后立即 start 的竞态路径。
- `restart` 命令与 `upgrade` 命令复用同一个端口释放检测工具，避免两个生命周期入口行为分叉。

## 清理步骤

- 本用例只 source shell 函数、执行 dry-run 或使用 `mktemp -d` 临时数据目录，不产生持久化测试数据。
- `test_upgrade_restart_e2e.sh` 退出时会停止测试 daemon 并删除临时数据目录。
- 退出当前 shell 即可清理函数定义。

## 执行记录

| 日期 | 用例 | 命令 | 实际结果 |
|------|------|------|----------|
| 2026-05-25 | TC-IBOC-01 | `BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 run_post_install /tmp/bifrost-test-bin` | PASS：输出包含 `ca install`、`install-skill --tool all -y`、`start --daemon --yes` |
| 2026-05-25 | TC-IBOC-02 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：17 个断言通过，证书 -> skills -> start 顺序正确 |
| 2026-05-25 | TC-IBOC-03 | `BIFROST_INSTALL_POST_INSTALL=0 run_post_install /tmp/bifrost-test-bin` | PASS：输出 `Post-install setup skipped`，不包含任何 dry-run post-install 命令 |
| 2026-05-25 | TC-IBOC-04 | `BIFROST_INSTALL_AUTO_CERT=0 BIFROST_INSTALL_AUTO_SKILLS=0 BIFROST_INSTALL_AUTO_START=0 run_post_install /tmp/bifrost-test-bin` | PASS：分别输出 CA、skills、service startup skipped |
| 2026-05-25 | TC-IBOC-05 | `bash ./install-binary.sh --help` | PASS：help 包含 `--no-post-install`、`--no-install-cert`、`--no-install-skills`、`--no-start` 和环境变量 |
| 2026-05-29 | TC-IBOC-01 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：默认 dry-run 输出包含 `ca install`、`install-skill --tool all -y`、`start --daemon --yes` |
| 2026-05-29 | TC-IBOC-02 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：17 个断言通过，证书 -> skills -> start 顺序正确 |
| 2026-05-29 | TC-IBOC-03 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：`BIFROST_INSTALL_POST_INSTALL=0` 跳过全部 post-install 命令 |
| 2026-05-29 | TC-IBOC-04 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：证书、skills、服务启动分步 opt-out 均可单独跳过 |
| 2026-05-29 | TC-IBOC-05 | `bash e2e-tests/tests/test_install_binary_post_install.sh` | PASS：help 包含 post-install 参数和环境变量 |
| 2026-05-29 | TC-IBOC-06 | `bash e2e-tests/tests/test_install_binary_adaptive_download.sh` | PASS：stub 模拟 GitHub 直连不可用时自动选择 `ghfast.top`，latest redirect race 和 selected source download 断言通过 |
| 2026-05-29 | TC-IBOC-07 | `bash e2e-tests/tests/test_install_binary_adaptive_download.sh` | PASS：stub 模拟最快源完整下载失败后回退全镜像竞速，help 包含 `BIFROST_MIRROR_PROBE_TIMEOUT` |
| 2026-05-29 | TC-IBOC-08 | `TMP_INSTALL_DIR=$(mktemp -d) ... bash install-binary.sh --no-post-install --no-modify-path` | PASS：真实 latest 探测安装 v0.0.84 到临时目录，archive 经 github.com 下载、checksum 经 ghfast.top 下载，校验通过，`bifrost --version` 输出 `bifrost 0.0.84`，临时目录已清理 |
| 2026-05-29 | TC-IBOC-09 | `pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1` | 未执行：当前 Mac 环境无 `pwsh` / `powershell`，命令返回 `zsh: command not found: pwsh`；已补测试脚本并通过源码 review，需 Windows/PowerShell 环境补跑 |
| 2026-05-29 | TC-IBOC-10 | `grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml && grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml && grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml && grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/release.yml && grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/release.yml && grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/release.yml` | PASS：CI 和 Release workflow 均设置 Cargo HTTP/2 multiplexing 关闭、10 次重试、120 秒 timeout |
| 2026-06-03 | TC-IBOC-10 | `bash e2e-tests/tests/test_install_binary_adaptive_download.sh` | PASS：8 个顶层用例通过，包含 `curl --progress-bar` 默认可见和竞速候选 `BIFROST_DOWNLOAD_PROGRESS=0` 安静模式 |
| 2026-06-03 | TC-IBOC-11 | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli upgrade_ --lib` | PASS：9 个 upgrade 相关测试通过，覆盖进度格式、镜像 URL 拼接、镜像展示名、下载 env 参数解析和 script install 输出继承终端 |
| 2026-06-03 | TC-IBOC-12 | `grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml && grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml && grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml && grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/release.yml && grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/release.yml && grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/release.yml` | PASS：CI 和 Release workflow 均保留 Cargo HTTP/2 multiplexing 关闭、10 次重试、120 秒 timeout |
| 2026-06-10 | TC-IBOC-06 / TC-IBOC-07 | `bash e2e-tests/tests/test_install_binary_adaptive_download.sh` | PASS：8 个顶层用例通过；selected source 与 fallback race 用例改为单镜像离线 stub，避免 fake release asset 在 CI macOS shell shard 中触发真实 mirror HEAD 探测 |
| 2026-06-11 | TC-IBOC-13 | `BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source ./install-binary.sh` + 本地 fake release `.tar.xz`/`.tar.gz` stub 安装 | PASS：`get_archive_ext_candidates darwin` 输出 `tar.xz` 后跟 `tar.gz`；首次安装优先命中 `.tar.xz`，模拟 `.tar.xz` 下载失败后自动回退 `.tar.gz` |
| 2026-06-11 | TC-IBOC-14 | `grep -q ... Cargo.toml .github/workflows/release.yml install-binary.sh crates/bifrost-cli/src/commands/upgrade.rs scripts/npm-publish.mjs` | PASS：release profile 设置 `strip = "symbols"`；Release workflow 产出并上传 `.tar.xz`；installer、内置 upgrade 和 npm publish 均有候选/兼容机制 |
| 2026-06-11 | TC-IBOC-15 | `grep -q ... .github/workflows/release.yml` | PASS：release job 在 npm publish 前检查 Unix/macOS `.tar.gz` + `.tar.xz`、Windows `.zip`；Homebrew 仍读取 macOS `.tar.gz` checksum |
| 2026-06-11 | TC-IBOC-16 | `BIFROST_DISABLE_XZ_ARCHIVE=1 get_archive_ext_candidates darwin`、旧 release 仅 `.tar.gz` 本地 HTTP mock 调用 `install_binary_for_target`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash ./install-binary.sh --version v0.0.96 --no-post-install --no-modify-path` 临时目录安装、`cargo test -p bifrost-cli upgrade_archive --lib -- --nocapture`、本地 Node tar mock | PASS：Bash installer 禁用 xz 后只返回 `.tar.gz`；旧 release 只有 `.tar.gz` 时新脚本先尝试可选 `.tar.xz`，探测不到后不进入全镜像竞速，立即回退 `.tar.gz` 并显示 curl 进度，checksum verified，临时安装的 `bifrost --version` 输出 `bifrost 0.0.96`；内置 upgrade 单测覆盖 `.tar.xz -> .tar.gz`、坏 `.tar.xz` 校验失败、禁用 xz、Windows zip；npm artifact mock 验证 `.tar.gz` 与 `.tar.xz` 都可提取二进制 |
| 2026-06-13 | TC-IBOC-17 | `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib upgrade_`、`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli --lib wait_for_port_released`、`bash e2e-tests/tests/test_upgrade_restart_e2e.sh` | PASS：upgrade 过滤下 13 个单测通过，覆盖 restart 端口选择；端口释放 helper 2 个单测通过，覆盖空闲快速返回与占用超时；upgrade restart E2E 14/14 PASS，源码门禁确认 stop 后等待端口释放、包含占用进程诊断、端口超时放弃重启时执行系统代理恢复，且端口释放 guard 与 wait helper 覆盖 Windows |
