# Tray 与 WebView 后台升级真实场景测试

## 功能模块说明

验证托盘/Admin/WebView 触发的后台升级链路，重点覆盖下载已完成、磁盘二进制已更新但运行中的 daemon 仍是旧版本时，后台 `self-update` 必须重启旧进程，WebView 也不能在进度被清理后一直卡在 Working 状态。

## 前置条件

1. 使用当前工作区编译出的 Bifrost 二进制，所有测试必须使用临时 `BIFROST_DATA_DIR`，避免污染正式服务数据。
2. 启动测试服务时设置：
   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   BIFROST_DISABLE_TRAY=1
   ```
3. 除非用例明确验证系统代理，否则启动服务必须带 `--no-system-proxy`。
4. 对 9900 现场排查只允许读取状态、日志和 API 响应；不得重启或停止用户当前 9900 服务。

## 测试用例

### TC-TWA-01：9900 现场版本错位诊断

**操作步骤**：
1. 查询 9900 端口监听进程，记录 PID 和启动命令。
2. 请求 `http://127.0.0.1:9900/_bifrost/api/system/overview`，记录运行中服务版本。
3. 执行 `/Users/eden/.local/bin/bifrost --version`，记录磁盘二进制版本。
4. 请求 `http://127.0.0.1:9900/_bifrost/api/system/version-check?refresh=true`，记录 `current_version`、`latest_version`、`has_update`。
5. 查询 `http://127.0.0.1:9900/_bifrost/api/system/upgrade/progress`，并检查 `~/.bifrost/logs/bifrost.log` 中最近一次 `admin upgrade: spawned self-update subprocess` 记录和对应子进程状态。

**预期结果**：
- 9900 服务仍可响应 Admin API。
- 若运行中版本小于磁盘版本，结论必须明确标注为“磁盘二进制已更新但旧 daemon 未重启”。
- 若 self-update 子进程已退出且旧 daemon 仍在，结论必须指向后台 latest 分支未重启，而不是下载卡住。
- 不修改 9900 服务状态。

### TC-TWA-02：后台 self-update 在磁盘二进制已是 latest 时仍重启 daemon

**操作步骤**：
1. 使用临时目录复制当前 `target/release/bifrost` 或 `target/debug/bifrost` 到测试安装路径。
2. 使用该安装路径启动 daemon：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   <tmp>/install/bifrost start -p <free-port> --host 127.0.0.1 --daemon --access-mode allow_all --skip-cert-check --no-system-proxy --no-intercept -y
   ```
3. 记录旧 PID。
4. 将 `BIFROST_UPGRADE_TEST_LATEST_VERSION` 设置为当前磁盘二进制版本，执行：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data \
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
   BIFROST_DISABLE_TRAY=1 \
   BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
   BIFROST_UPGRADE_TEST_LATEST_VERSION=<current-version> \
   <tmp>/install/bifrost self-update --target <current-version> --source admin
   ```
5. 等待 Admin API 恢复，读取新 PID 和 `<tmp>/data/upgrade-progress.json`。

**预期结果**：
- `self-update` 命令退出码为 0。
- 新 PID 与旧 PID 不同，且新 PID 存活。
- `upgrade-progress.json` 的 `phase` 为 `completed`。
- 测试服务可以正常 stop 并释放端口。

### TC-TWA-03：Admin API 后台升级完整 download/install/restart 链路

**操作步骤**：
1. 执行 `bash e2e-tests/tests/test_upgrade_admin_api_restart_e2e.sh`。
2. 观察 `POST /_bifrost/api/system/upgrade` 是否返回 active progress。
3. 观察 progress 是否到达 `completed`。
4. 观察重启后 PID 是否变化，运行路径是否仍指向测试安装路径。

**预期结果**：
- Admin POST upgrade 被接受，source 为 `admin`。
- progress 最终为 `completed`。
- 重启后 PID 变化，Admin API 恢复可访问。
- 新增的 already-latest 子用例也通过。

### TC-TWA-04：WebView active upgrade 遇到 idle progress 后退出 Working 状态

**操作步骤**：
1. 将 `useVersionStore` 置为 `upgrading=true` 且 `upgradePhase=installing`。
2. 模拟 `GET /system/upgrade/progress` 返回 `phase=idle`。
3. 运行升级轮询。
4. 检查 store 状态和 version-check 调用。

**预期结果**：
- `upgrading` 变为 `false`。
- `upgradePhase` 变为 `idle`。
- 无连接断开时，触发一次 `checkVersion({ forceRefresh: true, skipCache: true })`。
- 页面不会继续显示 Updating/Working 的升级弹窗。

### TC-TWA-05：后台升级子进程诊断日志与退出回收

**操作步骤**：
1. 触发一次 Admin 或 `self-update --source admin` 后台升级。
2. 检查测试数据目录下 `logs/upgrade-background.log`。
3. 查询后台升级子进程退出后是否仍有由 daemon 持有的 defunct child。

**预期结果**：
- 能在 `upgrade-background.log` 中看到后台升级输出或错误。
- 父进程存活且子进程退出时，不留下长期 defunct child。

## 清理步骤

1. 停止测试数据目录中的 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR=<tmp>/data <tmp>/install/bifrost stop
   ```
2. 删除临时测试目录。
3. 不清理、不停止、不重启用户正在运行的 9900 服务。

## 执行记录

待本次修复验证后更新。
