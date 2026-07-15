# 微信 Bot DNS 容错

## 功能模块说明

验证 macOS 系统 `getaddrinfo` 对微信 iLink 域名超时时，Bifrost 微信 provider 仍可通过 Hickory DNS 完成 DNS、TLS 和 HTTP 请求；测试不完成扫码、不发送消息。

## 前置条件

- 已构建当前 checkout 的 `target/debug/bifrost`。
- 网络能够直接连接 `ilinkai.weixin.qq.com:443`。
- 临时服务使用动态非 9900 端口、临时 `.bifrost-e2e-*` 数据目录、`--no-system-proxy`，禁用托盘与 Sync 登录弹窗。

## 测试用例列表

### TC-WDR-01：确认现场问题仅位于系统解析器

操作步骤：

1. 使用 `dig ilinkai.weixin.qq.com A` 获取 A 记录。
2. 对其中一个地址执行：

   ```bash
   curl --noproxy '*' \
     --resolve ilinkai.weixin.qq.com:443:<resolved-ip> \
     -o /dev/null -w '%{http_code}\n' \
     https://ilinkai.weixin.qq.com/
   ```

3. 对比不带 `--resolve` 的同一请求和正式服务 poll 日志。

预期结果：

- 直接 DNS 查询返回地址，指定地址的 TLS/HTTP 请求返回响应。
- 系统解析器异常时，普通 curl 和旧正式服务表现为域名解析超时。
- 证据说明网络和证书可用，故障边界在系统解析路径。

### TC-WDR-02：当前 Bifrost 通过 Hickory 请求微信登录起始接口

操作步骤：

1. 在临时数据目录与动态端口启动当前 debug 二进制。
2. 创建一个未配置 token、关闭长连接的临时微信 provider。
3. 请求 `POST /_bifrost/api/im-gateway/providers/<id>/weixin-login/start`。
4. 只断言返回 `success=true`、HTTPS `scan_url` 和正数过期时间；不展示二维码、不完成扫码。
5. 停止精确 PID 并清理临时目录。

预期结果：

- 请求在 20 秒内完成，没有 DNS 或 TLS 错误。
- 没有完成登录，没有发送任何测试消息。
- 临时服务和临时数据均完成清理，正式 9900 服务不受影响。

## 清理步骤

- 停止测试启动的精确 PID。
- 删除测试专属 `.bifrost-e2e-*` 临时目录。
- 不修改 `/etc/hosts`、系统 DNS、正式 provider 或正式扫码凭证。

## 执行记录

| 日期 | 用例 | 实际结果 | 结论 |
|---|---|---|---|
| 2026-07-16 | TC-WDR-01 | `dig` 返回 4 个 A 记录；逐个 `curl --resolve` 均完成 TLS 并返回 HTTP 404；普通 curl 在系统解析阶段超时，正式 poll 同样报 request network error | 通过 |
| 2026-07-16 | TC-WDR-02 | 当前 debug 二进制在动态端口和临时目录中成功返回微信登录二维码元数据；未扫码、未发送消息，临时进程与目录已清理 | 通过 |
