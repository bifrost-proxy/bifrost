# 二进制安装脚本一键体验真实场景测试

## 功能模块说明

验证 `install-binary.sh` 和 `install-binary.ps1` 在远程二进制安装时会自动探测更快的 GitHub/mirror 下载源，并在安装完成后默认规划并执行证书安装/信任、全量 skill 安装和 Bifrost 服务启动，形成一键安装、一键体验流程。为避免真实测试修改系统证书、skills 目录或系统代理，本用例使用脚本内置 dry-run post-install 路径和离线网络 stub 验证用户可感知命令编排。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 除 TC-IBOC-08 外，不下载 release；所有用例都不启动真实 Bifrost 服务，不修改系统代理。
- 所有用例都不安装真实 CA 证书，不写入真实 AI tool skills 目录。
- 下载源自适应用例通过 shell stub 模拟网络探测和下载结果，不访问真实 GitHub 或镜像。
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

### TC-IBOC-10 CI Cargo 依赖下载 HTTP/2 抖动回归

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

## 清理步骤

- 本用例只 source shell 函数和执行 dry-run，不产生持久化测试数据。
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
