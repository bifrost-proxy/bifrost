# E2E Runner Windows Stack

## 功能模块说明

`crates/bifrost-e2e` 提供 `bifrost-e2e` 测试入口。当前 GitHub Actions 的 Windows x86_64 / aarch64 E2E Runner job 会在执行到约 76/350 个测试后，让 `bifrost-e2e.exe` 主线程触发 `STATUS_STACK_OVERFLOW (0xc00000fd)`，导致整批 runner 用例在业务断言前直接崩溃。

## 实现逻辑

- 保持 `run(args)` 作为统一异步入口，避免平台差异渗入测试编排逻辑。
- 在 Windows 上不再直接依赖默认主线程栈运行 tokio runtime，而是先解析参数，再启动一个显式 `stack_size = 8 MiB` 的专用线程承载 runtime 与全部 runner 执行。
- 非 Windows 平台继续沿用原有 `#[tokio::main]` 入口，尽量缩小影响面。
- 增加一个轻量回归测试，确保 Windows 栈预算常量不会被误改小。

## 依赖项

- `crates/bifrost-e2e/src/main.rs`
- GitHub Actions `CI` workflow 中的 Windows E2E Runner job

## 测试方案

### 单元测试

- `cargo test -p bifrost-e2e windows_main_thread_stack_is_increased`：验证 Windows 专用主线程栈预算常量不低于 8 MiB。

### E2E 测试

- `cargo run -p bifrost-e2e -- --test remote_shell_exec_unix_shell_path_fallback --jobs 1 --port 18180`：验证入口改造后 runner 仍可正常启动并执行单个测试。
- 推送后使用 GitHub Actions PAT skill 观察 PR #111 的最新 `CI` run，重点确认 `E2E Runner (x86_64-pc-windows-msvc)` 与 `E2E Runner (aarch64-pc-windows-msvc)` 不再出现 `thread 'main' has overflowed its stack`。

### 真实场景测试

- 新增 `human_tests/e2e-runner-windows-stack.md`。
- 用例覆盖：
  - Windows 入口确实切换到大栈线程；
  - 本地单元测试与最小 runner smoke 通过；
  - 远端 PR CI 的两个 Windows runner job 不再出现栈溢出。

## 校验要求

- `cargo test -p bifrost-e2e windows_main_thread_stack_is_increased`
- `cargo run -p bifrost-e2e -- --test remote_shell_exec_unix_shell_path_fallback --jobs 1 --port 18180`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `bash scripts/ci/local-ci.sh --skip-e2e`
- 最终执行 `rust-project-validate`

## 文档更新要求

- 新增 `design/e2e-runner-windows-stack.md`
- 新增 `human_tests/e2e-runner-windows-stack.md`
- 更新 `human_tests/readme.md`
