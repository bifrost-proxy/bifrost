# 管理端虚拟 Host 测试用例

## 功能模块说明

验证 `bifrost.local` 管理端虚拟 Host 在真实代理链路中的行为。`http://bifrost.local/` 与 `https://bifrost.local/` 通过 Bifrost 代理访问时，应等价于访问当前实例的管理端首页，而不是按普通外部域名解析或转发。

## 前置条件

1. 使用当前分支构建出的 `target/debug/bifrost` 或 `target/release/bifrost`。
2. 测试启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`。
3. 测试启动 Bifrost 时必须设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，并携带 `--no-system-proxy`。
4. 测试通过显式 `curl -x http://127.0.0.1:<proxy_port>` 访问代理，不修改系统代理。

## 测试用例列表

### TC-AVH-01：不带端口的 `bifrost.local` 代理访问管理端

操作步骤：

1. 启动临时 Bifrost 服务，监听 `127.0.0.1:<proxy_port>`。
2. 执行 `curl --compressed -x http://127.0.0.1:<proxy_port> http://bifrost.local/`。

预期结果：

- 请求在 10 秒内返回 `200`。
- 响应正文为 Bifrost 管理端 HTML，包含 `Bifrost`。
- 请求不会超时，也不会尝试把 `bifrost.local` 当作普通 DNS 目标转发。

### TC-AVH-02：带当前代理端口的 `bifrost.local` 仍访问管理端

操作步骤：

1. 复用临时 Bifrost 服务。
2. 执行 `curl --compressed -x http://127.0.0.1:<proxy_port> http://bifrost.local:<proxy_port>/`。

预期结果：

- 请求返回 `200`。
- 响应正文为 Bifrost 管理端 HTML，包含 `Bifrost`。

### TC-AVH-03：HTTPS `bifrost.local` 代理访问管理端

操作步骤：

1. 复用临时 Bifrost 服务。
2. 执行 `curl -k --compressed -x http://127.0.0.1:<proxy_port> https://bifrost.local/`。

预期结果：

- 请求在 10 秒内返回 `200`。
- 响应正文为 Bifrost 管理端 HTML，包含 `Bifrost`。
- 代理对 `CONNECT bifrost.local:443` 启用管理虚拟 Host TLS intercept，并将解包后的请求交给管理端。

### TC-AVH-04：`Host: bifrost.local` 直连当前实例仍访问管理端

操作步骤：

1. 复用临时 Bifrost 服务。
2. 执行 `curl --compressed -H 'Host: bifrost.local' http://127.0.0.1:<proxy_port>/`。

预期结果：

- 请求返回 `200`。
- 响应正文为 Bifrost 管理端 HTML，包含 `Bifrost`。

### TC-AVH-05：默认系统代理 bypass 不排除 `bifrost.local`

操作步骤：

1. 复用临时 Bifrost 服务。
2. 执行 `curl http://127.0.0.1:<proxy_port>/_bifrost/api/proxy/system`。

预期结果：

- 响应中的默认 `configured_bypass` 包含 `localhost,127.0.0.1,::1`。
- 响应中的默认 `configured_bypass` 不包含 `*.local`。
- 浏览器使用系统代理时不会因为默认 bypass 绕过 Bifrost 后直接解析 `bifrost.local`。

### TC-AVH-06：普通外部 absolute-form 请求仍通过代理转发

操作步骤：

1. 启动一个本地 HTTP target，监听 `127.0.0.1:<target_port>` 并返回 `ordinary-target-ok`。
2. 复用临时 Bifrost 服务。
3. 执行 `curl -x http://127.0.0.1:<proxy_port> http://127.0.0.1:<target_port>/ordinary-target`。

预期结果：

- 请求返回 `200`。
- 响应正文为 `ordinary-target-ok`。
- 普通代理目标不会被误路由到管理端。

## 执行记录

| 日期 | 用例 | 命令 | 结果 |
| --- | --- | --- | --- |
| 2026-07-01 | TC-AVH-01 ~ TC-AVH-06 | `CARGO_TARGET_DIR=/Users/eden/work/github/bifrost/target BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost e2e-tests/tests/test_admin_virtual_host_proxy.sh` | PASS：`http://bifrost.local/`、`https://bifrost.local/`、`http://bifrost.local:<proxy_port>/` 和 `Host: bifrost.local` 直连均返回 Bifrost 管理端 HTML；默认系统代理 bypass 不包含 `*.local`；普通代理目标 `ordinary-target-ok` 仍正确转发；脚本使用临时数据目录、`--no-system-proxy` 并完成清理。 |
| 2026-07-01 | TC-AVH-01 ~ TC-AVH-06 / SKIP_BUILD 复用路径 | `bash -n e2e-tests/tests/test_admin_virtual_host_proxy.sh`；`CARGO_TARGET_DIR=/Users/eden/work/github/bifrost/target SKIP_BUILD=true BIFROST_DISABLE_TRAY=1 BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=/Users/eden/work/github/bifrost/target/debug/bifrost e2e-tests/tests/test_admin_virtual_host_proxy.sh` | PASS：脚本语法检查通过；已构建 binary 复用路径仍完整通过 HTTP、HTTPS、默认 bypass、直连 Host header 和普通代理目标验证。 |

## 清理步骤

1. 停止临时 Bifrost 进程。
2. 停止本地 HTTP target 进程。
3. 删除临时 `BIFROST_DATA_DIR`。
4. 确认没有修改系统代理配置。
