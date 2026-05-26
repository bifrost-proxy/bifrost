# 二进制安装脚本一键体验真实场景测试

## 功能模块说明

验证 `install-binary.sh` 在远程二进制安装完成后，默认会自动规划并执行证书安装/信任、全量 skill 安装和 Bifrost 服务启动，形成一键安装、一键体验流程。为避免真实测试修改系统证书、skills 目录或系统代理，本用例使用脚本内置 dry-run post-install 路径验证用户可感知命令编排。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不下载 release，不启动真实 Bifrost 服务，不修改系统代理。
- 不安装真实 CA 证书，不写入真实 AI tool skills 目录。
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
