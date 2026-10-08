# 同端口远端管理请求回归

## 功能模块说明

验证监听通配地址的 Bifrost 能转发同端口远端 IP 的管理路径，同时保留本机管理端防伪检查。仅确认不属于本机网卡的 IP 字面量为远端；同端口域名和网卡枚举失败仍采用原有保守行为。

## 前置条件

1. 构建当前分支：`SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost`。本用例只验证 API，不依赖前端资产。
2. 使用独立 `.bifrost-e2e-*` 数据目录及非 9900 测试端口；设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`，启动携带 `--no-system-proxy`。
3. 在独立端口启动 HTTP 目标，使 `/_bifrost/api/rules` 返回 `remote-admin-target`。设置 `BIFROST_ROUTING_TARGET=127.0.0.1:<目标端口>`。
4. 使用 `e2e-tests/rules/forwarding/admin_same_port.txt` 启动代理，监听 `0.0.0.0:<代理端口>`。文档保留地址 `192.0.2.80` 通过规则转发到目标，不需要真实外网服务。

## 测试用例列表

### TC-ASP-01：本机直连 API

执行 `curl --noproxy '*' -fsS http://127.0.0.1:<代理端口>/_bifrost/api/auth/status`。

预期：200 且响应为有效 JSON。

### TC-ASP-回归-02：同端口远端管理路径

执行 `curl --noproxy '' -fsS -x http://127.0.0.1:<代理端口> http://192.0.2.80:<代理端口>/_bifrost/api/rules`。

预期：200 且正文为 `remote-admin-target`。旧版本在本机管理请求防伪检查处返回 403；修复后请求进入正常代理规则并到达目标。

### TC-ASP-03：本机 absolute-form 防伪检查

执行 `curl --noproxy '' -sS -o /dev/null -w '%{http_code}' -x http://127.0.0.1:<代理端口> http://127.0.0.1:<代理端口>/_bifrost/api/rules`。

预期：403，不能借代理形式访问本机管理 API。

### TC-ASP-04：管理虚拟 Host

执行 `curl --noproxy '' -fsS -x http://127.0.0.1:<代理端口> http://bifrost.local:<代理端口>/_bifrost/api/auth/status`。

预期：200 且响应为有效 JSON。

## 清理步骤

按记录的 PID 停止本次代理与 HTTP 目标，等待子进程退出后删除本次临时目录。禁止清理正式 9900 端口、按进程名批量杀进程或修改正式系统代理。
