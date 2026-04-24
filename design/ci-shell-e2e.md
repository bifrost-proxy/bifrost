# CI Shell E2E

## 功能模块说明

CI shell E2E 通过 `scripts/ci/run-e2e-shell.sh` 调用 `scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build`，并由 `BIFROST_E2E_SHARD_INDEX` / `BIFROST_E2E_SHARD_TOTAL` 在 CI runner 间分片执行。

系统代理用例 `test_system_proxy_e2e.sh` 会修改宿主机网络代理设置，在 macOS CI 的临时 runner 上存在系统设置收敛不稳定问题。该用例不再纳入 CI shell 集合，仅保留本地 `--full-shell` 场景执行。

## 实现逻辑

- `scripts/run_all_e2e.sh` 的 `SKIP_IN_CI_TESTS` 维护 CI 禁跑脚本列表。
- `collect_shell_tests` 在 `MODE=ci` 时过滤 `SKIP_IN_CI_TESTS`，过滤后再应用分片，避免被跳过用例占用 shard 槽位。
- `test_system_proxy_e2e.sh` 加入 `SKIP_IN_CI_TESTS` 后，`scripts/ci/run-e2e-shell.sh` 在 macOS/Linux/Windows CI 中均不会收集该脚本。
- 本地运行 `bash scripts/run_all_e2e.sh --full-shell ...` 时 `MODE=local`，不会应用 CI skip 列表，仍可手动验证系统代理功能。
- `--list-shell-tests` 只打印当前 mode/shard 下会被收集的 shell 脚本并退出，用于验证调度结果，不会构建、启动 Bifrost 或修改系统代理配置。

## 依赖项

- `scripts/run_all_e2e.sh`
- `scripts/ci/run-e2e-shell.sh`
- `e2e-tests/tests/test_system_proxy_e2e.sh`

## 测试方案

### 单元测试

本次修改为 Bash 调度逻辑，无 Rust 公共函数变化，不新增 Rust 单元测试。通过脚本级命令验证 `collect_shell_tests` 的 CI 过滤行为。

### E2E 测试

- 运行 `bash scripts/run_all_e2e.sh --ci --full-shell --list-shell-tests --shard 3/3`，断言输出中没有 `test_system_proxy_e2e.sh`。
- 运行 `bash scripts/run_all_e2e.sh --full-shell --list-shell-tests`，断言本地 full-shell 仍可收集 `test_system_proxy_e2e.sh`。

### 真实场景测试

- 更新 `human_tests/ci-shell-e2e-sharding.md`，新增 CI 不执行系统代理用例的回归用例。
- 按新增用例逐条执行，确认 CI 模式过滤、本地模式保留。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`

## 文档更新要求

- 更新 `human_tests/ci-shell-e2e-sharding.md`
- 更新 `human_tests/readme.md`
