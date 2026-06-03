# E2E Script Startup

## 功能模块说明

验证 E2E 脚本和 Rust integration test 启动 Bifrost 测试服务时默认禁用 Sync 自动登录弹窗，避免本地执行和 CI 执行用例时打开登录页面、污染用户环境或干扰自动化判断。

## 前置条件

- 在仓库根目录执行。
- 脚本扫描范围包括 `e2e-tests/**/*.sh`、`scripts/**/*.sh` 和 `tests/**/*.sh`。
- Rust 扫描范围包括 `crates/**/*.rs` 和 `tests/**/*.rs` 中直接启动 `CARGO_BIN_EXE_bifrost` 的测试。

## 测试用例列表

### TC-ESS-01：Bifrost 测试启动入口默认禁用 Sync 登录弹窗

**操作步骤**：
1. 执行静态守卫脚本：
   ```bash
   bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh
   ```
2. 检查脚本输出。

**预期结果**：
- `e2e-tests/test_utils/process.sh` 默认导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 直接启动 Bifrost 且不走公共 helper 的脚本显式导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- Rust integration test 直接启动 `CARGO_BIN_EXE_bifrost start` 时显式设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 专门验证 Sync 启动登录预检的 `test_sync_startup_login_preflight_e2e.sh` 是唯一例外。
- 脚本输出 `All Bifrost startup tests/scripts disable Sync auto-login prompt by default.`。

**本轮执行记录（2026-06-03）**：
- 已执行 `bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`。
- 扫描结果通过，输出 `All E2E Bifrost startup scripts disable Sync auto-login prompt by default.`。
- 本轮扩大扫描范围后，补齐顶层 `e2e-tests/*.sh`、`scripts/**/*.sh` 以及不依赖公共 helper 的 `e2e-tests/tests/*.sh` 启动入口；`test_sync_startup_login_preflight_e2e.sh` 作为唯一验证 Sync 启动登录预检的例外保留。

**补充执行记录（2026-06-03）**：
- 用户反馈 `SKIP_FRONTEND_BUILD=1 cargo test --workspace --all-features -- --test-threads=1` 仍会打开登录页后，补充扫描 Rust integration test 入口。
- 已补齐 `crates/bifrost-cli/tests/daemon_shutdown.rs` 的真实 `bifrost start --daemon` 启动命令，默认设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 已重新执行 `bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`，输出 `All Bifrost startup tests/scripts disable Sync auto-login prompt by default.`。

### TC-ESS-02：Sync 登录预检专用脚本不受父环境默认禁用污染

**操作步骤**：
1. 在父环境显式设置默认禁用变量，并使用 dry-run 文件执行 Sync 登录预检专用脚本：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_BUILD=true BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh
   ```
2. 检查脚本输出。

**预期结果**：
- reachable remote case 通过 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE` 记录一次登录 URL，不真实打开浏览器。
- restart case 不重复记录登录 URL。
- environment disables startup login prompt case 显式设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 后不记录登录 URL。
- unreachable remote case 不记录登录 URL。
- 脚本输出 `[sync-startup-preflight] PASS`。

**执行记录（2026-06-03）**：
- 已执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_BUILD=true BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh`。
- 输出依次覆盖 reachable remote、restart、environment disables startup login prompt、unreachable remote 四个 case，并以 `[sync-startup-preflight] PASS` 结束。
- 该验证使用 `BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE` 记录登录 URL，不真实打开浏览器。

## 清理步骤

本用例只做静态扫描，不启动 Bifrost，不产生临时服务进程。
