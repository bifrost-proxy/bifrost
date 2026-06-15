# Rule TLS Intercept Routing Exceptions

## 功能模块说明

验证规则命中后的自动 TLS 解包边界：

- 带具体域名/IP 作用域的 `host://`、路径级 `http://` / `https://` 等路由规则应自动开启 TLS 解包，即使 matcher 没有协议前缀，以便内层 HTTPS 请求能继续参与优先级匹配。
- 纯 `proxy://` 下游代理规则只改变代理出口，不应自动开启 TLS 解包，且流量必须真实到达下游代理。
- `proxy://` 是严格例外：即使 matcher 有具体域名或路径，只要目标协议只有下游代理转发，也不应因 proxy 规则自动 TLS 解包。
- 纯 regex 或纯 wildcard 响应规则范围过大，不应作为规则驱动自动 TLS 解包的理由。
- 内容改写类规则仍应在全局 TLS 解包关闭时自动开启 TLS 解包，以便规则能读写内层 HTTP。
- `passthrough://` 与转发/host 路由遵循同一套 first-win 语义，低优先级 passthrough 不应覆盖更高优先级的具体转发目标。

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

### TC-S5TRE-01 SOCKS5 proxy-only 规则不自动 TLS 解包且真实走下游代理

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

### TC-S5TRE-02 HTTP CONNECT proxy-only 规则不自动 TLS 解包且真实走下游代理

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

### TC-S5TRE-03 纯 wildcard 响应规则不自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `* resHeaders://X-Bifrost-Wildcard-Auto-TLS=unexpected`。
2. 通过 SOCKS5 端口请求 `https://localhost:<HTTPS_PORT>/health`。
3. 检查响应头、上游 `/_bifrost/api/traffic?limit=100` 和上游代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头不包含 `X-Bifrost-Wildcard-Auto-TLS`。
- Traffic 中不存在 `host=localhost` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=localhost` 的 `TLS interception enabled` 记录。

### TC-S5TRE-04 纯 regex 响应规则不自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `/localhost/ resHeaders://X-Bifrost-Regex-Auto-TLS=unexpected`。
2. 通过 SOCKS5 端口请求 `https://localhost:<HTTPS_PORT>/health`。
3. 检查响应头、上游 `/_bifrost/api/traffic?limit=100` 和上游代理日志。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头不包含 `X-Bifrost-Regex-Auto-TLS`。
- Traffic 中不存在 `host=localhost` 且 `protocol=https` 的解包记录。
- 日志中不存在 `original_host=localhost` 的 `TLS interception enabled` 记录。

### TC-S5TRE-05 内容改写规则仍自动 TLS 解包

操作步骤：

1. 同一脚本写入规则 `socks-mutation.local http://127.0.0.1:<HTTP_PORT> resHeaders://X-Bifrost-Auto-TLS=socks5-content-rule`。
2. 通过 SOCKS5 端口请求 `https://socks-mutation.local/health`。
3. 检查响应头和 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 响应头包含 `X-Bifrost-Auto-TLS: socks5-content-rule`。
- Traffic 中存在 `host=socks-mutation.local` 且 `protocol=https` 的解包记录。

### TC-S5TRE-06 无协议前缀的具体域名路径路由自动 TLS 解包并命中最具体规则

操作步骤：

1. 同一脚本写入规则：

   ```txt
   domain-path-auto.local/app/account-center https://127.0.0.1:<SECONDARY_HTTPS_PORT>
   domain-path-auto.local/app https://domain-path-auto.local/app
   domain-path-auto.local https://127.0.0.1:<ECHO_HTTPS_PORT>
   ```

2. 确认上游 Bifrost 使用 `--no-intercept` 启动，且未配置 `--intercept-include domain-path-auto.local`。
3. 通过 HTTP proxy 端口请求 `https://domain-path-auto.local/app/account-center`。
4. 检查响应 body 中的 `server.port`、`request.path`，以及 `/_bifrost/api/traffic?limit=100`。

预期结果：

- HTTPS 请求返回 `200`。
- 响应 body 中 `server.port` 等于第一条规则的 `<SECONDARY_HTTPS_PORT>`，而不是兜底规则的 `<ECHO_HTTPS_PORT>`。
- 响应 body 中 `request.path` 等于 `/app/account-center`。
- Traffic 中存在 `host=domain-path-auto.local` 且 `protocol=https` 的解包记录，证明该具体域名路径路由自动开启 TLS 解包。

### TC-S5TRE-07 用户反馈请求中低优先级 passthrough 不覆盖高优先级路径转发

操作步骤：

1. 使用用户反馈导出的网络请求文件 `/Users/eden/Downloads/bifrost-network-20260615T093213.bifrost`。
2. 解析导出的请求，确认第一条记录：

   - `url = https://qianchuan.jinritemai.com/app/account-center`
   - `host = qianchuan.jinritemai.com`
   - `path = /app/account-center`

3. 使用同一用户反馈规则验证 resolver 与 request rules：

   ```txt
   qianchuan.jinritemai.com/app/account-center https://10.37.102.138:8081
   qianchuan.jinritemai.com/app https://qianchuan.jinritemai.com/app
   qianchuan.jinritemai.com https://10.37.102.138:8080
   ```

4. 执行聚焦单测：

   ```bash
   cargo test -p bifrost-admin test_passthrough_does_not_override_higher_priority_forward_rule --all-features -- --nocapture
   ```

预期结果：

- 导出请求确认为用户反馈的 `qianchuan.jinritemai.com/app/account-center` HTTPS 请求。
- 第二条规则仍会被 parser 规范成 `passthrough://`，但不会覆盖第一条更高优先级的 `https://10.37.102.138:8081`。
- applied routing 的 `forward_url` 为 `https://10.37.102.138:8081`，`forwarding_passthrough=false`。

### TC-S5TRE-08 routing exceptions fixture 不进入通用 rules runner

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
- 专用脚本的 7 个场景全部通过。
- 并行 rules runner 的 socks5_tls 分类测试通过，不再出现 CI 中的 `socks5_tls/routing_exceptions.txt` 失败断言。

实际结果：

- 通过。2026-06-03 按项目规则带 `source ~/.zshrc;` 执行 `rg -n 'socks5_tls/routing_exceptions.txt' e2e-tests/run_all_tests_parallel.sh`，确认该 fixture 已列入 `FIXTURE_ONLY_RULES`。
- 通过。执行 `bash -n e2e-tests/run_all_tests_parallel.sh && bash -n e2e-tests/tests/test_socks5_tls_routing_exceptions.sh` 退出码为 0。
- 通过。2026-06-04 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`，TC-S5TRE-01/02/03/04/05/06 全部通过；其中 TC-S5TRE-02/03 均验证上游不产生 HTTPS 解包记录、不出现 TLS interception 日志，且下游 Bifrost proxy 记录并记录日志显示目标 CONNECT；TC-S5TRE-04/05 验证纯 wildcard / regex 响应规则请求成功但不自动解包、不应用响应头；TC-S5TRE-06 验证明确域名内容规则仍自动解包并应用响应头。
- 通过。2026-06-15 执行 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_BIN=./target/debug/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`，TC-S5TRE-06 验证无协议前缀的 `domain-path-auto.local/app/account-center` 自动解包 HTTPS、保留内层 path，并转发到第一条最具体规则的 secondary HTTPS 上游。
- 通过。2026-06-15 解析用户反馈文件 `/Users/eden/Downloads/bifrost-network-20260615T093213.bifrost`，确认请求 URL、host、path 与 TC-S5TRE-07 一致；执行 `cargo test -p bifrost-admin test_passthrough_does_not_override_higher_priority_forward_rule --all-features -- --nocapture`，确认低优先级 passthrough 不覆盖第一条高优先级 HTTPS 转发。
- 通过。执行 `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once`，该分类没有通用 runner 可执行文件并以退出码 0 收敛。随后执行 `BIFROST_E2E_RULE_JOBS=4 BIFROST_E2E_RETRY_FAILED_ONCE=1 SKIP_FRONTEND_BUILD=1 e2e-tests/run_all_tests_parallel.sh --no-build --retry-failed-once`，66/66 套件通过、575/575 断言通过，失败数 0。

## 清理步骤

测试脚本通过 `trap cleanup EXIT` 自动执行：

- 停止 Bifrost 测试进程。
- 停止 mock HTTP/HTTPS 服务和上下游 Bifrost 测试代理。
- 删除 `.bifrost-e2e-socks5-tls-routing-*` 临时数据目录。
