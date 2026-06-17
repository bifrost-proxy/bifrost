# E2E Script Startup

## 功能模块详细描述

E2E 脚本启动 Bifrost 测试服务时必须避免打开 Sync 自动登录页面。测试脚本通常只需要本地代理、Admin API 或 mock 服务链路，默认弹出登录浏览器会污染本机环境，也会干扰 CI 自动化。

## 实现逻辑

- `e2e-tests/test_utils/process.sh` 默认导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`（使用 `: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"` 形式，便于父环境覆盖），覆盖所有 source 公共 process helper 或 admin client helper 的脚本。
- 不走公共 helper、但可能直接启动 Bifrost 的 shell 脚本（命中 `BIFROST_BIN`、`target/{debug,release}/bifrost`、`cargo run --bin bifrost` 等启动 hint），在文件开头显式导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 直接通过 `CARGO_BIN_EXE_bifrost` 调用 `.arg("start")` 的 Rust integration test（例如 `crates/bifrost-cli/tests/daemon_shutdown.rs`）也必须在 `Command` 环境中显式设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- `e2e-tests/tests/test_sync_startup_login_preflight_e2e.sh` 是唯一例外（写入静态守卫脚本的 `ALLOWLIST`），它专门验证 Sync 启动登录预检和禁用开关，因此由脚本自身按 case 显式控制该变量。

## 依赖项

- E2E shell 脚本（`e2e-tests/**/*.sh`、`scripts/**/*.sh`、`tests/**/*.sh`）。
- `e2e-tests/test_utils/process.sh`、`e2e-tests/test_utils/admin_client.sh`（间接引入默认禁用）。
- 直接启动 `CARGO_BIN_EXE_bifrost` 的 Rust integration test（`crates/**/*.rs`、`tests/**/*.rs`）。

## 测试方案

- E2E 测试：执行 `e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`，静态扫描 `e2e-tests/**/*.sh`、`scripts/**/*.sh`、`tests/**/*.sh`，以及 `crates/**/*.rs`、`tests/**/*.rs` 中调用 `CARGO_BIN_EXE_bifrost` + `.arg("start")` 的 Rust integration test，并校验 `process.sh` 保留 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1` 默认值。脚本通过时输出 `All Bifrost startup tests/scripts disable Sync auto-login prompt by default.`。
- 真实场景测试：更新并执行 `human_tests/e2e-script-startup.md` 的 `TC-ESS-01` 与 `TC-ESS-02`（后者覆盖 Sync 登录预检专用脚本不被父环境默认禁用污染）。

## Review/Fix/Test 闭环方案

- 第 1 轮：扫描所有 Bifrost 启动脚本，修复公共 helper 和直接脚本缺口，运行静态守卫脚本。
- 第 2 轮：复查 allowlist、bash 语法、WebSocket 相关脚本 E2E 和 human_tests/readme 索引。

## 校验要求

- 所有修改过的 shell 脚本必须通过 `bash -n`。
- 静态守卫脚本必须通过。
- 专门的 Sync 登录预检脚本不得被公共默认值破坏。

## 文档更新要求

- 更新 `human_tests/e2e-script-startup.md`。
- 更新 `human_tests/readme.md` 索引。
