# Rule TLS Intercept Routing Exceptions

## 功能模块说明

验证规则命中后的自动 TLS 解包边界：

- 纯 `host://` 路由规则只改变目标地址，不应自动开启 TLS 解包。
- 纯 `proxy://` 下游代理规则只改变代理出口，不应自动开启 TLS 解包。
- 内容改写类规则仍应在全局 TLS 解包关闭时自动开启 TLS 解包，以便规则能读写内层 HTTP。

## 前置条件

1. 在仓库根目录执行。
2. 已构建最新 Bifrost 二进制：

   ```bash
   cargo build --release --bin bifrost --all-features
   ```

3. 测试启动服务时必须使用临时数据目录、禁用系统代理、禁用 Sync 自动登录弹窗：

   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```

## 测试用例列表

### TC-S5TRE-01 host-only 规则不自动 TLS 解包

操作步骤：

1. 启动脚本 `e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`。
2. 脚本写入规则 `socks-host-only.local host://127.0.0.1:<HTTPS_PORT>`。
3. 通过 SOCKS5 端口请求 `https://socks-host-only.local/health`。
4. 查询 `/_bifrost/api/traffic?limit=100` 并检查代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- Traffic 中不存在 `host=socks-host-only.local` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=socks-host-only.local` 的 `TLS interception enabled` 记录。

### TC-S5TRE-02 proxy-only 规则不自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `127.0.0.1 proxy://127.0.0.1:<PROXY_PORT>`。
2. 通过 SOCKS5 端口请求 `https://127.0.0.1:<HTTPS_PORT>/health`。
3. 查询 `/_bifrost/api/traffic?limit=100` 并检查代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- Traffic 中不存在 `host=127.0.0.1` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=127.0.0.1` 的 `TLS interception enabled` 记录。

### TC-S5TRE-03 内容改写规则仍自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `socks-mutation.local http://127.0.0.1:<HTTP_PORT> resHeaders://X-Bifrost-Auto-TLS=socks5-content-rule`。
2. 通过 SOCKS5 端口请求 `https://socks-mutation.local/health`。
3. 检查响应头和 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头包含 `X-Bifrost-Auto-TLS: socks5-content-rule`。
- Traffic 中存在 `host=socks-mutation.local` 且 `protocol=https` 的解包记录。

## 清理步骤

测试脚本通过 `trap cleanup EXIT` 自动执行：

- 停止 Bifrost 测试进程。
- 停止 mock HTTP/HTTPS/proxy 服务。
- 删除 `.bifrost-e2e-socks5-tls-routing-*` 临时数据目录。
