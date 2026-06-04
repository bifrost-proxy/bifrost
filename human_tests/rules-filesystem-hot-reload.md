# 规则文件系统热更新真实场景测试

## 功能模块说明

验证运行中的 Bifrost 在未登录远端 Sync 的本地使用场景下，可以感知 CLI 本地规则命令和直接文件修改，并让代理运行时规则、Rules active summary、页面小圆点使用的 Badge 规则快照保持一致。

## 前置条件

- 在仓库根目录执行。
- 已编译最新 `target/debug/bifrost`，或允许测试脚本自动执行 `cargo build --bin bifrost`。
- 测试启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- 测试默认设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免未登录 Sync 时打开登录浏览器。

## 测试用例列表

### TC-RFHR-01 CLI 本地规则新增后运行时自动生效

操作步骤：
1. 执行 `BIFROST_BIN=target/debug/bifrost e2e-tests/tests/test_rules_filesystem_hot_reload.sh`。
2. 脚本启动真实 HTTP echo server 和真实 Bifrost 代理。
3. 脚本在同一个 `BIFROST_DATA_DIR` 下执行 `bifrost rule add <name> --content "127.0.0.1 statusCode://203"`。
4. 脚本不调用 WebUI 保存或 Admin API 创建规则，直接通过代理请求 echo server。
5. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- CLI 命令成功写入规则文件。
- 代理请求状态码自动从 `200` 变为 `203`。
- active summary 的 `merged_content` 包含 `statusCode://203`。

### TC-RFHR-02 直接编辑 `.bifrost` 文件后运行时自动更新

操作步骤：
1. 继续执行同一个脚本。
2. 脚本先执行 `bifrost rule update <name> --content "127.0.0.1 statusCode://204"`。
3. 脚本不调用 WebUI 保存或 Admin API 更新规则，直接通过代理请求 echo server。
4. 脚本读取 `/_bifrost/api/rules/active-summary`。
5. 脚本直接修改 `rules/<name>.bifrost`，将 `statusCode://204` 替换为 `statusCode://205`。
6. 脚本不调用 WebUI 保存或 Admin API 更新规则，直接通过代理请求 echo server。
7. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- CLI update 后代理请求状态码自动变为 `204`。
- CLI update 后 active summary 的 `merged_content` 包含 `statusCode://204`。
- 直接文件编辑后代理请求状态码自动变为 `205`。
- 直接文件编辑后 active summary 的 `merged_content` 包含 `statusCode://205`。
- 说明页面小圆点依赖的 Badge 规则快照已随磁盘变化刷新。

### TC-RFHR-03 CLI 本地规则删除后运行时自动清理

操作步骤：
1. 继续执行同一个脚本。
2. 脚本执行 `bifrost rule delete <name>`。
3. 脚本不调用 WebUI 保存或 Admin API 删除规则，直接通过代理请求 echo server。
4. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- 代理请求状态码恢复为原始 `200`。
- active summary 不再包含被删除的规则名。

### TC-RFHR-04 直接删除 `.bifrost` 文件后运行时自动清理

操作步骤：
1. 继续执行同一个脚本。
2. 脚本重新执行 `bifrost rule add <name> --content "127.0.0.1 statusCode://206"`。
3. 脚本直接删除 `rules/<name>.bifrost`。
4. 脚本不调用 WebUI 保存或 Admin API 删除规则，直接通过代理请求 echo server。
5. 脚本读取 `/_bifrost/api/rules/active-summary`。

预期结果：
- 重新添加后代理请求状态码自动变为 `206`。
- 直接删除文件后代理请求状态码恢复为原始 `200`。
- active summary 不再包含被删除的规则名。

## 清理步骤

- 脚本退出时停止 Bifrost 代理进程。
- 脚本退出时停止 HTTP echo server。
- 脚本退出时删除临时 `BIFROST_DATA_DIR`。
