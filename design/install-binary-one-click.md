# 二进制安装脚本一键体验

## 功能模块说明

`install-binary.sh` 是用户通过 `curl ... | bash` 使用的官方远程二进制安装入口。安装完成后，脚本默认继续完成一键体验初始化：

- 安装并信任 Bifrost CA 证书。
- 安装所有支持 AI 工具的 Bifrost skills。
- 启动 Bifrost 服务，用户安装完成后可直接访问默认管理端和代理能力。

目标是把原先“安装二进制后再手动处理证书、skill、启动服务”的多步流程合并为一次安装命令。高级用户和 CI 仍可通过参数或环境变量跳过自动步骤。

## 实现逻辑

- `install-binary.sh` 下载并安装 CLI 后调用 `run_post_install "$INSTALL_DIR/$binary_name"`。
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

- `install-binary.sh`
- `crates/bifrost-cli/src/commands/ca.rs`
- `crates/bifrost-cli/src/commands/install_skill.rs`
- `crates/bifrost-cli/src/commands/start.rs`
- `e2e-tests/tests/test_install_binary_post_install.sh`
- `human_tests/install-binary-one-click.md`

## 测试方案

### 单元测试

- 本次修改为 shell installer 编排逻辑，不新增 Rust 公共函数。
- 使用 `bash -n install-binary.sh` 覆盖 shell 语法。

### E2E 测试

- 新增 `e2e-tests/tests/test_install_binary_post_install.sh`：
  - source `install-binary.sh` 并设置 `BIFROST_INSTALL_BINARY_SKIP_MAIN=1`，避免真实下载 release。
  - 设置 `BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1`，验证默认命令顺序为 `ca install` -> `install-skill --tool all -y` -> `start --daemon --yes`。
  - 验证 `BIFROST_INSTALL_POST_INSTALL=0` 不执行任何 post-install 命令。
  - 验证 `BIFROST_INSTALL_AUTO_CERT=0`、`BIFROST_INSTALL_AUTO_SKILLS=0`、`BIFROST_INSTALL_AUTO_START=0` 可分别跳过证书、skills、启动。
  - 验证 `--help` 展示 post-install opt-out 参数和环境变量。

### 真实场景测试

- 新增 `human_tests/install-binary-one-click.md`：
  - 默认 dry-run 输出包含证书安装、全量 skill 安装和服务启动命令。
  - 验证命令顺序符合一键体验目标。
  - 验证全局 opt-out 和分步 opt-out。
  - 验证 help 文案可发现。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：默认证书、skills、服务启动是否全部覆盖。
- 复核变更范围：`git status --short`、`git diff`，确认未触碰既有 im-gateway 改动。
- 代码 review：检查 `install-binary.sh` 在 `PATH` 未刷新、权限失败、CI opt-out、dry-run 下的行为。
- 复测命令：`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`。

### 第 2 轮

- 再次对照第 1 轮 diff 和测试输出，检查文档、E2E、human_tests/readme 是否同步。
- 复测命令：`bash -n install-binary.sh`、`bash e2e-tests/tests/test_install_binary_post_install.sh`、human_tests 中列出的 dry-run 命令。

## 校验要求

- `bash -n install-binary.sh`
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
