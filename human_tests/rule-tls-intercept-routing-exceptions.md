# Rule TLS Intercept Routing Exceptions

## 功能模块说明

验证规则命中后的自动 TLS 解包边界：

- 纯 `host://` 路由规则只改变目标地址，不应自动开启 TLS 解包。
- 纯 `proxy://` 下游代理规则只改变代理出口，不应自动开启 TLS 解包，且流量必须真实到达下游代理。
- 纯 regex 或纯 wildcard 响应规则范围过大，不应作为规则驱动自动 TLS 解包的理由。
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

### TC-S5TRE-02 SOCKS5 proxy-only 规则不自动 TLS 解包且真实走下游代理

操作步骤：

1. 同一脚本启动上游 SOCKS5 Bifrost 和下游 HTTP Bifrost proxy。
2. 同一脚本写入规则 `127.0.0.1 proxy://127.0.0.1:<DOWNSTREAM_PROXY_PORT>`。
3. 通过上游 SOCKS5 端口请求 `https://127.0.0.1:<HTTPS_PORT>/health`。
4. 查询上游 `/_bifrost/api/traffic?limit=100` 并检查上游代理日志。
5. 查询下游 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 上游 Traffic 中不存在 `host=127.0.0.1` 且 `protocol=https` 的解包记录。
- 上游日志中不存在 `original_host=127.0.0.1` 的 `TLS interception enabled` 记录。
- 下游 Traffic 中存在目标 `127.0.0.1:<HTTPS_PORT>` 的 CONNECT/隧道记录，证明流量实际经过下游代理。

### TC-S5TRE-03 HTTP CONNECT proxy-only 规则不自动 TLS 解包且真实走下游代理

操作步骤：

1. 同一脚本启动上游 HTTP/SOCKS5 Bifrost 和下游 HTTP Bifrost proxy。
2. 同一脚本写入规则 `127.0.0.1 proxy://127.0.0.1:<DOWNSTREAM_PROXY_PORT>`。
3. 通过上游 HTTP proxy 端口请求 `https://127.0.0.1:<HTTPS_PORT>/health`。
4. 查询上游 `/_bifrost/api/traffic?limit=100` 并检查上游代理日志。
5. 查询下游 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 上游 Traffic 中不存在 `host=127.0.0.1` 且 `protocol=https` 的解包记录。
- 上游日志中不存在 `TLS interception enabled for 127.0.0.1` 记录。
- 下游 Traffic 中存在目标 `127.0.0.1:<HTTPS_PORT>` 的 CONNECT/隧道记录，证明流量实际经过下游代理。

### TC-S5TRE-04 纯 wildcard 响应规则不自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `wildcard-auto.local host://127.0.0.1:<HTTPS_PORT>` 和 `* resHeaders://X-Bifrost-Wildcard-Auto-TLS=unexpected`。
2. 通过 SOCKS5 端口请求 `https://wildcard-auto.local/health`。
3. 检查响应头、上游 `/_bifrost/api/traffic?limit=100` 和上游代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头不包含 `X-Bifrost-Wildcard-Auto-TLS`。
- Traffic 中不存在 `host=wildcard-auto.local` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=wildcard-auto.local` 的 `TLS interception enabled` 记录。

### TC-S5TRE-05 纯 regex 响应规则不自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `regex-auto.local host://127.0.0.1:<HTTPS_PORT>` 和 `/regex-auto\.local/ resHeaders://X-Bifrost-Regex-Auto-TLS=unexpected`。
2. 通过 SOCKS5 端口请求 `https://regex-auto.local/health`。
3. 检查响应头、上游 `/_bifrost/api/traffic?limit=100` 和上游代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头不包含 `X-Bifrost-Regex-Auto-TLS`。
- Traffic 中不存在 `host=regex-auto.local` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=regex-auto.local` 的 `TLS interception enabled` 记录。

### TC-S5TRE-06 内容改写规则仍自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `socks-mutation.local http://127.0.0.1:<HTTP_PORT> resHeaders://X-Bifrost-Auto-TLS=socks5-content-rule`。
2. 通过 SOCKS5 端口请求 `https://socks-mutation.local/health`。
3. 检查响应头和 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头包含 `X-Bifrost-Auto-TLS: socks5-content-rule`。
- Traffic 中存在 `host=socks-mutation.local` 且 `protocol=https` 的解包记录。

### TC-S5TRE-07 routing exceptions fixture 不进入通用 rules runner

操作步骤：

1. 确认 `routing_exceptions.txt` 已被并行 rules runner 归类为 fixture-only：

   ```bash
   rg -n 'socks5_tls/routing_exceptions.txt' e2e-tests/run_all_tests_parallel.sh
   ```

2. 执行 shell 语法检查：

   ```bash
   bash -n e2e-tests/run_all_tests_parallel.sh e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```

3. 执行专用脚本验证真实 SOCKS5 TLS routing exceptions 行为：

   ```bash
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_BUILD=true SKIP_FRONTEND_BUILD=1 e2e-tests/tests/test_socks5_tls_routing_exceptions.sh
   ```

4. 执行缩小范围的并行 rules runner，确认通用 runner 不再误套 `routing_exceptions.txt` 的混合规则语义：

   ```bash
   BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once
   ```

预期结果：

- `routing_exceptions.txt` 出现在 `FIXTURE_ONLY_RULES` 中。
- shell 语法检查通过。
- 专用脚本的 6 个场景全部通过。
- 并行 rules runner 的 socks5_tls 分类测试通过，不再出现 CI 中的 `socks5_tls/routing_exceptions.txt` 失败断言。

实际结果：

- 通过。2026-06-03 按项目规则带 `source ~/.zshrc;` 执行 `rg -n 'socks5_tls/routing_exceptions.txt' e2e-tests/run_all_tests_parallel.sh`，确认该 fixture 已列入 `FIXTURE_ONLY_RULES`。
- 通过。执行 `bash -n e2e-tests/run_all_tests_parallel.sh && bash -n e2e-tests/tests/test_socks5_tls_routing_exceptions.sh` 退出码为 0。
- 通过。2026-06-04 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`，TC-S5TRE-01/02/03/04/05/06 全部通过；其中 TC-S5TRE-02/03 均验证上游不产生 HTTPS 解包记录、不出现 TLS interception 日志，且下游 Bifrost proxy 记录并记录日志显示目标 CONNECT；TC-S5TRE-04/05 验证纯 wildcard / regex 响应规则请求成功但不自动解包、不应用响应头；TC-S5TRE-06 验证明确域名内容规则仍自动解包并应用响应头。
- 通过。执行 `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once`，该分类没有通用 runner 可执行文件并以退出码 0 收敛。随后执行 `BIFROST_E2E_RULE_JOBS=4 BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 e2e-tests/run_all_tests_parallel.sh --no-build --retry-failed-once`，66/66 套件通过、575/575 断言通过，失败数 0。

## 清理步骤

测试脚本通过 `trap cleanup EXIT` 自动执行：

- 停止 Bifrost 测试进程。
- 停止 mock HTTP/HTTPS 服务和上下游 Bifrost 测试代理。
- 删除 `.bifrost-e2e-socks5-tls-routing-*` 临时数据目录。
