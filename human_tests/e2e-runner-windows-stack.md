# E2E Runner Windows Stack

## 功能模块说明

验证 `bifrost-e2e` 在 Windows CI 上不再因为默认主线程栈过小而触发 `thread 'main' has overflowed its stack`，并确认入口调整没有破坏本地 runner 基本执行能力。

## 前置条件

1. 在仓库根目录执行。
2. 不使用 9900 端口。
3. 如需本地运行 runner，使用非 9900 端口，例如 `18180`。
4. 访问 GitHub API 时使用代理隔离：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY=
   ```

## 测试用例列表

### TC-ERWS-01：Windows 入口切换到大栈线程

**操作步骤**：
1. 检查 Windows 专用入口是否显式创建大栈线程：
   ```bash
   rg -n 'WINDOWS_MAIN_THREAD_STACK_SIZE|stack_size\(|bifrost-e2e-main|tokio::runtime::Builder::new_multi_thread' crates/bifrost-e2e/src/main.rs
   ```

**预期结果**：
- 能看到 `WINDOWS_MAIN_THREAD_STACK_SIZE` 常量。
- 能看到 `stack_size(WINDOWS_MAIN_THREAD_STACK_SIZE)`。
- 能看到 Windows 分支在专用线程里构建 tokio runtime，而不是直接复用默认主线程。

### TC-ERWS-02：本地单元测试与 runner smoke 通过

**操作步骤**：
1. 执行针对性单元测试：
   ```bash
   cargo test -p bifrost-e2e windows_main_thread_stack_is_increased
   ```
2. 执行最小 runner smoke：
   ```bash
   cargo run -p bifrost-e2e -- --test remote_shell_exec_unix_shell_path_fallback --jobs 1 --port 18180
   ```

**预期结果**：
- 第 1 步通过，断言 Windows 栈预算常量不少于 8 MiB。
- 第 2 步输出单个测试通过，不出现入口初始化失败、tokio runtime 初始化失败或栈溢出。

### TC-ERWS-03：PR #111 的 Windows E2E Runner 不再栈溢出

**操作步骤**：
1. 推送包含本修复的 commit 到 `fix/ci-rules-tls-readiness`。
2. 查询 PR #111 最新 run：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
   GH_REPO=bifrost-proxy/bifrost \
   python3 .agents/skills/github-actions-pat/scripts/gh_ci.py pr 111 --any-status
   ```
3. 使用 fail-fast 方式监控新 run，必要时继续修复：
   ```bash
   NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
   HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= \
   GH_REPO=bifrost-proxy/bifrost \
   POLL_SEC=20 MAX_WAIT_SEC=3600 \
   python3 .agents/skills/github-actions-pat/scripts/watch_jobs.py <new_run_id>
   ```

**预期结果**：
- `E2E Runner (x86_64-pc-windows-msvc)` 不再出现 `thread 'main' has overflowed its stack`。
- `E2E Runner (aarch64-pc-windows-msvc)` 不再出现 `thread 'main' has overflowed its stack`。
- 如 run 仍失败，失败原因必须转移到新的、可继续定位的问题，而不是同一栈溢出。

## 清理步骤

1. 如本地执行了 runner smoke，无需额外服务清理；若遗留临时目录，可删除测试产生的临时目录。
2. 保持工作区不使用 9900 端口。

## 执行记录

- 2026-05-06：TC-ERWS-01 通过。执行 `rg -n 'WINDOWS_MAIN_THREAD_STACK_SIZE|stack_size\(|bifrost-e2e-main|tokio::runtime::Builder::new_multi_thread' crates/bifrost-e2e/src/main.rs`，确认 Windows 分支显式创建 `bifrost-e2e-main` 大栈线程并在其中构建 tokio runtime。
- 2026-05-06：TC-ERWS-02 通过。执行 `cargo test -p bifrost-e2e windows_main_thread_stack_is_increased`，测试通过；执行 `cargo run -p bifrost-e2e -- --test remote_shell_exec_unix_shell_path_fallback --jobs 1 --port 18180`，单个 runner smoke 通过，未出现入口初始化失败或栈溢出。
- 2026-05-06：TC-ERWS-03 待本轮 commit push 后继续执行，需以新的 PR #111 CI run 结果确认两个 Windows Runner job 不再出现 `thread 'main' has overflowed its stack`。
