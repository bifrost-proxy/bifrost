# E2E Script Startup

## 功能模块说明

验证 E2E 脚本启动 Bifrost 测试服务时默认禁用 Sync 自动登录弹窗，避免本地执行和 CI 执行脚本用例时打开登录页面、污染用户环境或干扰自动化判断。

## 前置条件

- 在仓库根目录执行。
- 脚本扫描范围包括 `e2e-tests/**/*.sh`、`scripts/**/*.sh` 和 `tests/**/*.sh`。

## 测试用例列表

### TC-ESS-01：E2E 脚本启动 Bifrost 默认禁用 Sync 登录弹窗

**操作步骤**：
1. 执行静态守卫脚本：
   ```bash
   bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh
   ```
2. 检查脚本输出。

**预期结果**：
- `e2e-tests/test_utils/process.sh` 默认导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 直接启动 Bifrost 且不走公共 helper 的脚本显式导出 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。
- 专门验证 Sync 启动登录预检的 `test_sync_startup_login_preflight_e2e.sh` 是唯一例外。
- 脚本输出 `All E2E Bifrost startup scripts disable Sync auto-login prompt by default.`。

**本轮执行记录（2026-06-03）**：
- 已执行 `bash e2e-tests/tests/test_e2e_scripts_disable_sync_login_prompt.sh`。
- 扫描结果通过，输出 `All E2E Bifrost startup scripts disable Sync auto-login prompt by default.`。
- 本轮扩大扫描范围后，补齐顶层 `e2e-tests/*.sh`、`scripts/**/*.sh` 以及不依赖公共 helper 的 `e2e-tests/tests/*.sh` 启动入口；`test_sync_startup_login_preflight_e2e.sh` 作为唯一验证 Sync 启动登录预检的例外保留。

## 清理步骤

本用例只做静态扫描，不启动 Bifrost，不产生临时服务进程。
