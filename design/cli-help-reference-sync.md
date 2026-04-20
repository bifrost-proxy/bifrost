# CLI Help 参考文案同步

## 背景

`bifrost --help` 的 `SUBCOMMAND REFERENCE` 区块是手写静态文案，不会自动跟随 `clap` 的 `start --help` 输出更新。近期 `start` 子命令已经新增或调整了 `--no-system-proxy`、`--proxy-bypass`、`--cli-proxy`、`--cli-proxy-no-proxy`、`-y/--yes` 等参数说明，但顶层参考区块没有完整同步，导致用户看到的命令参考与实际可用参数不一致。

## 修复目标

- 让顶层 `bifrost --help` 中的 `start [OPTIONS]` 参考区块与实际 `bifrost start --help` 保持一致
- 明确补上 `--no-system-proxy`
- 统一关键参数描述，避免用户根据过时帮助文案遗漏可用参数

## 实现方案

- 更新 `crates/bifrost-cli/src/cli.rs` 中 `after_help` 的 `start [OPTIONS]` 静态文案
- 将 `--skip-cert-check`、`--access-mode`、`--proxy-user`、`--no-intercept`、`--unsafe-ssl`、`--system-proxy` 的描述同步到当前 `clap` 定义语义
- 在系统代理参数组中加入 `--no-system-proxy`
- 保留 `--proxy-bypass`、`--cli-proxy`、`--cli-proxy-no-proxy`、`-y/--yes`，确保顶层参考完整

## 测试方案

### 单元/CLI 测试

- `crates/bifrost-cli/tests/cli_commands.rs`
- 新增回归测试：`root_help_start_reference_stays_in_sync_with_start_flags`
- 验证点：
  - 顶层 `bifrost --help` 包含 `start [OPTIONS]`
  - 顶层参考区块包含 `--no-system-proxy`
  - 顶层参考区块包含 `--proxy-bypass`、`--cli-proxy`、`--cli-proxy-no-proxy`、`-y, --yes`

### E2E 测试

- 本次为帮助文案修复，不新增独立脚本
- 使用现有 CLI 测试覆盖 `--help` 输出回归

### 真实场景测试（human_tests）

- 更新 `human_tests/cli-start-advanced.md`
- 新增用例：验证顶层 `bifrost --help` 的 `start` 参考区块包含 `--no-system-proxy`，并与 `bifrost start --help` 的关键参数集合保持一致
- 同步更新 `human_tests/readme.md` 用例数量

## 校验要求

- `cargo test -p bifrost-cli cli_commands::root_help_start_reference_stays_in_sync_with_start_flags -- --exact`
- `cargo test -p bifrost-cli start_all_options_parse -- --exact`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- `cargo test --workspace --all-features`
- 按项目要求在任务收尾前执行 rust-project-validate 规定项

## 文档更新

- 无需更新 README
- 需要更新 `human_tests/cli-start-advanced.md`
- 需要更新 `human_tests/readme.md`
