# CI Windows E2E Runner

## 功能模块说明

Windows `E2E Runner` job 负责在 `x86_64-pc-windows-msvc` 和 `aarch64-pc-windows-msvc` hosted runner 上执行 `scripts/ci/run-e2e-runner.sh`，该入口最终通过 `cargo run -p bifrost-e2e` 编译并运行自定义 E2E runner。

## 实现逻辑

- GitHub Actions 当前失败发生在 `E2E Runner (x86_64-pc-windows-msvc)` 的 `cargo run -p bifrost-e2e` 编译阶段。
- 失败日志显示多个 Rust 工具链路径在并行编译早期同时触发 `rust-src` 下载，随后 rustup 报 `failed to install component: 'rust-src', detected conflict: 'lib\rustlib\src\rust\library\Cargo.toml'`。
- Windows E2E runner 的 `dtolnay/rust-toolchain@stable` 步骤显式声明 `components: rust-src`，让组件安装在 cargo 并行编译前由单个 Actions step 完成。
- `scripts/run_all_e2e.sh` 在解析 `CARGO_BIN` 后，如果调用方未显式设置 `RUSTC`，会通过 `rustup which rustc` 绑定同一当前工具链的真实 `rustc` 路径，并在 E2E Runtime Context 打印 `Rustc bin`。这避免 Windows Git Bash `PATH` 解析或 rustup shim fallback 把 Cargo 1.95 和 Rustc 1.65 混用，导致 `--check-cfg` 被旧 rustc 当作 unstable flag。
- Linux/macOS runner 当前没有同类失败；本次仅收敛 Windows runner matrix，避免扩大 CI 变更面。

## 依赖项

- `.github/workflows/ci.yml`
- `scripts/ci/run-e2e-runner.sh`
- `scripts/run_all_e2e.sh`
- `crates/bifrost-e2e`

## 测试方案

### 单元测试

本次变更为 GitHub Actions workflow 配置，不修改 Rust 逻辑和公共函数，不新增 Rust 单元测试。通过 YAML 解析和 CI 实际运行验证。

### E2E 测试

- 静态解析 `.github/workflows/ci.yml`，确认 `e2e-windows-runner` job 存在。
- 检查该 job 的 `dtolnay/rust-toolchain@stable` step 显式包含 `components: rust-src`。
- 执行 `bash scripts/run_all_e2e.sh --ci --skip-rules --skip-shell --skip-runner --skip-ui --skip-build`，确认 Runtime Context 输出 Cargo/Rustc 路径且不启动任何 suite。
- 静态检查 `scripts/run_all_e2e.sh`，确认默认 `RUSTC` 来自 `rustup which rustc`，且 Runtime Context 输出 `Rustc bin`。
- 推送分支后观察 GitHub Actions `CI` workflow，确认 `E2E Runner (x86_64-pc-windows-msvc)` 不再在 `rust-src` component conflict 处失败。

### 真实场景测试

- 新增 `human_tests/ci-windows-e2e-runner.md`。
- 执行静态 workflow 和脚本检查，验证 Windows E2E runner 的 toolchain step 已预安装 `rust-src`，且 E2E 入口会显式绑定 `RUSTC`。
- 推送后使用 `github-actions-pat` 的 fail-fast watcher 观察最新 CI run。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核失败日志、workflow diff、设计文档和 human_tests；运行 YAML 解析与静态断言。
- 第 2 轮：复查第 1 轮修复后的最新 diff、索引同步和 CI watch 结果；必要时继续追加修复并重新推送观察。

## 校验要求

- `ruby -e 'require "yaml"; YAML.load_file(".github/workflows/ci.yml")'`
- 静态断言 `e2e-windows-runner` 的 toolchain step 包含 `components: rust-src`
- 静态断言 `scripts/run_all_e2e.sh` 通过 `rustup which rustc` 设置默认 `RUSTC`
- GitHub Actions 最新 `CI` run fail-fast watch

## 文档更新要求

- 更新 `human_tests/ci-windows-e2e-runner.md`
- 更新 `human_tests/readme.md`
