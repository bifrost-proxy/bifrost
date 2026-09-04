# Client Admin CLI 真实场景测试

## 功能模块说明

验证调用端使用 `bifrost client` 经目标设备的非 loopback 局域网 IP 直连 Admin API。覆盖目标保存与登录、单/多目标选择、代表性 Admin 查询和写入、流式流量能力、凭据失效恢复，以及与 Remote Invoke 和本机命令的隔离。

## 前置条件

1. 从当前源码立即编译 `bifrost`，不复用其他 checkout 的二进制。
2. 使用动态非 `9900` 端口和独立临时 `BIFROST_DATA_DIR` 启动目标实例。
3. 启动时设置 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，并传 `--no-system-proxy`。
4. 目标监听 `0.0.0.0`，测试 URL 必须使用当前机器可用的非 `127.0.0.1` IPv4 地址。
5. 在目标数据目录设置 Admin 密码并从目标机本地执行 `bifrost admin remote enable`。
6. 使用 `e2e-tests/tests/test_client_admin_cli.sh` 执行本页用例；脚本通过 PID 树和临时目录标记做定向清理，不接触正式 `9900` 实例。

## 测试用例

### TC-CLIENT-01 保存目标并经 LAN 登录

操作步骤：

1. 将目标的非 loopback `http://<lan-ip>:<dynamic-port>` 保存为 `lan`，显式允许可信局域网 HTTP。
2. 在目标尚未开启 Remote Access 时尝试登录，再在目标机本地开启 Remote Access。
3. 分别使用错误密码和正确密码执行 `client target login lan --password-stdin`。
4. 检查 target list/show 以及调用端 profile 和 credential 文件。

预期结果：

- 未开启时不能从 Client 自举开关；错误密码返回认证失败；正确密码登录成功且请求没有被环境代理回送。
- profile 与 JWT 分文件保存；Unix credential 文件权限为 `0600`。
- 密码和 token 不出现在 argv 或普通 target profile。

### TC-CLIENT-02 单目标、多目标和显式选择

操作步骤：

1. 只有 `lan` 时执行 `bifrost client status --format json`。
2. 增加第二个 profile，在非 TTY 中省略 `--target` 再执行 status。
3. 执行 `bifrost client --target lan status --format json`。
4. 通过 `BIFROST_CLIENT_TARGET=lan` 选择目标，并重命名、删除第二个 profile。

预期结果：

- 单目标自动选择，返回端口等于动态目标端口。
- 多目标非 TTY 明确失败并要求 `--target`，不猜测目标。
- 显式 selector 成功且 stdout 保持原 status JSON schema。
- 环境 selector、target rename/remove 正常工作。

### TC-CLIENT-03 Admin 内容与配置读写

操作步骤：

1. 通过 Client 创建、读取并清理 rule、value、script、whitelist 和 proxy account。
2. 查询目标 config、metrics 和 Sync status。
3. 创建临时 port、读取其 active rule，再销毁该 port。

预期结果：

- 所有写入均能从目标 Admin API 回读，未写入调用端业务数据。
- config、metrics、Sync 与 port 生命周期返回目标实例状态。

### TC-CLIENT-04 Traffic、SSE Search 与 Capture

操作步骤：

1. 通过目标代理发送带唯一 marker 的 HTTP 请求。
2. 通过 Client 执行 traffic list 和 SSE search。
3. 启动 Client capture wait，再发送匹配请求。

预期结果：

- traffic list 返回目标记录。
- SSE search 返回 marker 对应结果且没有 401。
- authenticated capture long-poll 返回 `matched: true`。

### TC-CLIENT-05 本机与 Remote Invoke 隔离

操作步骤：

1. 执行 `bifrost client start`。
2. 执行 `bifrost client remote conn status`。

预期结果：

- 生命周期命令在 dispatch 前以 local-only 原因拒绝，不启动调用端服务。
- nested Remote Invoke 明确拒绝并提示是独立 transport，不发起 Relay 调用。

### TC-CLIENT-06 JWT 撤销、重新登录与本地注销

操作步骤：

1. 执行 Client Admin audit 和 `admin revoke-all`。
2. 使用旧凭据执行 status。
3. 显式重新登录，随后执行 `target logout` 并再次查询。

预期结果：

- audit 与 revoke-all 可通过目标 Admin API 完成。
- 撤销后的请求返回非零并提示运行 `client target login`，不自动改走本机或 Remote Invoke。
- 重新登录成功；logout 后调用端凭据删除，后续命令明确报告未登录。

### TC-CLIENT-07 安全启动与清理

操作步骤：

1. 检查测试脚本使用共享 process helper、动态端口、双环境护栏与 `--no-system-proxy`。
2. 测试结束后检查目标、echo server、capture 子进程和临时目录。

预期结果：

- 不使用正式 `9900`，不执行全机进程名清理。
- 本次 PID 树结束且临时目录被删除，不影响正式 Bifrost。

## 清理步骤

测试脚本的 `EXIT` trap 按记录的 PID 终止 capture、echo server 和目标实例，然后删除带 E2E ownership 标记的临时目录。若测试异常退出，先按 PID 检查残留；禁止使用 `pkill -f bifrost`、`killall bifrost` 或按正式 `9900` 端口清理。
