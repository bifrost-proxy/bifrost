# Daemon Certificate Check

## 功能模块说明

`bifrost start -d` 以前会跳过 CA 证书安装/信任检查，只在 daemon 子进程里生成本地 CA 文件。这会让异步启动看似成功，但用户第一次启用 TLS interception 或使用匹配到 TLS interception 的规则时，系统/浏览器因为没有信任 Bifrost CA 而失败。

本方案让 daemon 启动和前台启动共享同一套证书准备门禁，并且保证门禁发生在 fork daemon 之前。

## 实现逻辑

- `start` 默认执行 CA 证书检查，不再因为 `--daemon` 跳过。
- `--skip-cert-check` 保持原语义：显式跳过 CA 安装/信任检查。
- `--yes` 自动确认安装/信任证书，符合 CLI help 中“自动回答 yes”的语义。
- 如果 CA 未安装或未信任，且当前没有交互式终端、也没有 `--yes`，启动直接失败并给出修复提示：
  - 使用 `bifrost start --daemon --yes`
  - 先执行 `bifrost ca install`
  - 或在明确知道风险时加 `--skip-cert-check`
- daemon 子进程仍通过 `load_tls_config()` 确保 CA 文件存在，但不负责弹交互或安装系统 trust store。

## 依赖项

- `bifrost_tls::CertInstaller`
- `bifrost_tls::CertStatus`
- `std::io::IsTerminal`
- 现有 `--yes` 与 `--skip-cert-check` CLI 参数

## 测试方案

### 单元测试

- `certificate_resolution_blocks_non_interactive_missing_ca_without_yes`：无 TTY 且无 `--yes` 时，未安装 CA 必须被阻断。
- `certificate_resolution_auto_installs_when_yes`：`--yes` 对未安装/未信任 CA 都选择自动安装/信任。
- `certificate_resolution_prompts_when_terminal_available`：有交互式终端时仍走 prompt。

### E2E 测试

新增 `e2e-tests/tests/test_daemon_cert_check_e2e.sh`：

- 使用独立 `BIFROST_DATA_DIR`。
- 验证 `start --daemon` 在 stdin 为 `/dev/null`、CA 未安装且无 `--yes` 时失败，不报告 daemon 启动成功。
- 验证 `start --daemon --skip-cert-check` 在同类临时目录中仍能后台启动并暴露 Admin API，确认跳过参数不被破坏。

### 真实场景测试

更新 `human_tests/cli-start-stop-status.md`：

- 新增 daemon CA 检查回归用例，覆盖非交互失败与 `--skip-cert-check` 显式跳过。
- 执行真实 CLI 命令，不修改系统代理，使用临时数据目录和临时端口。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 `start.rs` 是否只改变 CA 检查时机，不影响端口冲突、运行中重启、daemon readiness。
- 复核 `ca.rs` 是否避免非交互静默跳过。
- 运行相关单元测试和新增 E2E。

### 第 2 轮

- 复查最新 diff、docs、human_tests 索引和已暂存的既有改动边界。
- 复跑受影响测试，确认 `--skip-cert-check` 和 daemon readiness 仍可用。

## 校验要求

- `cargo test -p bifrost-cli certificate_resolution`
- `bash e2e-tests/tests/test_daemon_cert_check_e2e.sh`
- `cargo test --workspace --all-features`
- 如时间和环境允许，最后执行 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。

## 文档更新要求

- 更新 `docs/cli.md` 的 `start` 参数说明。
- 更新 `docs/getting-started.md` 的启动说明。
- 更新 `human_tests/cli-start-stop-status.md` 和 `human_tests/readme.md`。
