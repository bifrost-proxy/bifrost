# 二进制安装脚本一键体验

## 功能模块说明

`install-binary.sh` 是用户通过 `curl ... | bash` 使用的官方远程二进制安装入口。安装完成后，脚本默认继续完成一键体验初始化：

- 安装并信任 Bifrost CA 证书。
- 安装所有支持 AI 工具的 Bifrost skills。
- 启动 Bifrost 服务，用户安装完成后可直接访问默认管理端和代理能力。

目标是把原先“安装二进制后再手动处理证书、skill、启动服务”的多步流程合并为一次安装命令。高级用户和 CI 仍可通过参数或环境变量跳过自动步骤。

## 实现逻辑

- `install-binary.sh` 下载并安装 CLI 后调用 `run_post_install "$INSTALL_DIR/$binary_name"`。
- Bash installer 下载 release 资产前会对 GitHub 直连和内置镜像源做轻量可用性探测，优先选择最快返回的源；如果被选中的源在完整下载阶段失败，再回退到所有镜像和下载器的竞速下载。
- PowerShell installer 使用同一组镜像候选源和短超时探测，latest、archive、checksums 都通过选出的最快可用源下载；如果选中源完整下载失败，则继续按候选源列表回退。
- 最新版本探测不再按 `github.com -> mirror` 串行等待完整超时；Bash 通过并发重定向探测抢最快结果，PowerShell 通过短超时探测先选源再读取 `releases/latest` 重定向，避免默认 GitHub 直连在受限网络中拖到完整下载超时。
- `BIFROST_GITHUB_MIRROR` 仍作为优先候选源保留，`BIFROST_DOWNLOAD_CONNECT_TIMEOUT`（Bash）、`BIFROST_DOWNLOAD_TIMEOUT`、`BIFROST_DOWNLOAD_TRIES` 继续控制下载；新增 `BIFROST_MIRROR_PROBE_TIMEOUT` 控制镜像轻量探测超时，默认 5 秒。
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
- `crates/bifrost-cli/src/commands/ca.rs`
- `crates/bifrost-cli/src/commands/install_skill.rs`
- `crates/bifrost-cli/src/commands/start.rs`
- `e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `e2e-tests/tests/test_install_binary_post_install.sh`
- `human_tests/install-binary-one-click.md`

## 测试方案

### 单元测试

- 本次修改为 shell installer 编排逻辑，不新增 Rust 公共函数。
- 使用 `bash -n install-binary.sh` 覆盖 shell 语法。

### E2E 测试

- 新增 `e2e-tests/tests/test_install_binary_adaptive_download.sh`：
  - source `install-binary.sh` 并设置 `BIFROST_INSTALL_BINARY_SKIP_MAIN=1`，避免真实下载 release。
  - stub `probe_github_url`，验证默认 GitHub 不可用时会选择 `https://ghfast.top/https://github.com`。
  - stub `get_latest_version_via_redirect`，验证最新版本探测使用并发最快镜像结果。
  - stub `download_file`，验证完整下载优先使用已探测出的最快源。
  - stub `download_github_file_race`，验证最快源完整下载失败后仍回退到旧的全镜像竞速路径。
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

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认证书、skills、服务启动是否全部覆盖。
- 复核变更范围：`git status --short`、`git diff`，确认未触碰既有 im-gateway 改动。
- 代码 review：检查 `install-binary.sh` 在 `PATH` 未刷新、权限失败、CI opt-out、dry-run 下的行为。
- 代码 review：检查镜像探测不会污染 `VERSION=$(get_latest_version)` stdout，不会破坏用户指定 `BIFROST_GITHUB_MIRROR`，被选中源失败后仍能回退旧下载路径；检查 PowerShell env 变量、latest、archive、checksum 下载路径与 Bash installer 保持一致。
- 复测命令：`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）。

### 第 2 轮

- 再次对照第 1 轮 diff 和测试输出，检查文档、E2E、human_tests/readme 是否同步。
- 复测命令：`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_adaptive_download.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、`pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`（若环境可用）、human_tests 中列出的 dry-run 命令。

## 校验要求

- `grep -q 'CARGO_HTTP_MULTIPLEXING: "false"' .github/workflows/ci.yml`
- `grep -q 'CARGO_NET_RETRY: "10"' .github/workflows/ci.yml`
- `grep -q 'CARGO_HTTP_TIMEOUT: "120"' .github/workflows/ci.yml`
- `bash -n install-binary.sh`
- `bash e2e-tests/tests/test_install_binary_adaptive_download.sh`
- `pwsh -NoProfile -File e2e-tests/tests/test_install_binary_windows_adaptive_download.ps1`
- `bash e2e-tests/tests/test_install_binary_post_install.sh`
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
