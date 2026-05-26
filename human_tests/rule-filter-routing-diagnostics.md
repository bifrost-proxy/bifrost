# Rule Filter Routing Diagnostics 真实场景测试

## 功能模块说明

验证规则过滤器的路径前缀匹配与网络包导出诊断字段。覆盖用户反馈的两类问题：

- `includeFilter:///account` 应按普通前缀匹配 `/account-center/...` 这类更长路径；需要路径段边界或其它复杂约束时应使用正则。
- qianchuan 类长 `excludeFilter` 链必须在同一条规则中全部生效；任意列出的路径前缀命中后都应跳过该规则，继续后续规则。
- 兼容 Whistle 风格 URL 通配符过滤器，如 `excludeFilter://*/api` 与 `excludeFilter://*/alice/*`；未带 `m:`、`reqH:` 等类型前缀的通配符过滤值按请求 URL pattern 匹配。
- `upstreamUnsafeSsl://true` 只对命中的规则允许不安全上游 HTTPS 证书，不需要启动 Bifrost 时全局开启 `--unsafe-ssl`。
- 网络 `.bifrost` 包必须导出 `actual_url`、`actual_host`、`listener_port`、`has_rule_hit` 等诊断字段，便于判断规则是否命中以及实际转发到了哪里。

## 前置条件

1. 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
2. 使用临时端口和临时数据目录启动 Bifrost，必须携带 `--no-system-proxy`；本用例不启用全局 `--unsafe-ssl`。
3. 本用例可直接执行自动化回归脚本：

```bash
cargo build --bin bifrost
BIFROST_BIN="$PWD/target/debug/bifrost" e2e-tests/tests/test_rule_filter_routing_diagnostics.sh
```

## 测试用例列表

### TC-RFRD-01：`includeFilter:///account` 前缀匹配 `/account-center`

操作步骤：

1. 启动本地 echo 上游服务。
2. 启动 Bifrost，规则文件内容为：

```text
filter-regression.local 127.0.0.1:${ECHO_HTTP_PORT} includeFilter:///account
```

3. 通过 Bifrost 代理访问：

```bash
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://filter-regression.local/account-center/account-assistant?from=e2e"
```

预期结果：

- 请求返回 2xx。
- echo 响应中的 `request.parsed_path` 为 `/account-center/account-assistant`。
- 该请求命中主规则，证明 `includeFilter:///account` 对 `/account-center/...` 使用普通前缀匹配。
- 如果要只匹配 `/account` 或 `/account/...`，应写正则过滤器，例如 `/^\/account(\/|$|\?)/`。

### TC-RFRD-02：Traffic 详情保留实际转发诊断字段

操作步骤：

1. 复用 TC-RFRD-01 产生的请求。
2. 调用 Admin API 获取 traffic 列表，找到该 URL 的记录 ID：

```bash
curl "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=50"
```

3. 调用详情接口：

```bash
curl "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic/${RECORD_ID}"
```

预期结果：

- `has_rule_hit` 为 `true`。
- `actual_host` 为 `127.0.0.1`。
- `actual_url` 包含 echo 上游端口。

### TC-RFRD-03：长 `excludeFilter` 链按前缀跳过当前规则

操作步骤：

1. 启动本地 echo 上游服务。
2. 启动 Bifrost，规则文件包含 qianchuan 风格的长 `excludeFilter` 链和一个兜底 host 规则：

```text
qianchuan.jinritemai.com 10.37.102.138:8080 excludeFilter:///account/page/cooperate/qianchuan excludeFilter:///garrmodlistv3 ... excludeFilter:///star-pages
qianchuan.jinritemai.com 127.0.0.1:${ECHO_HTTP_PORT}
```

3. 分别通过 Bifrost 代理访问：

```bash
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://qianchuan.jinritemai.com/account-center/account-assistant?from=e2e"
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://qianchuan.jinritemai.com/garrmodlistv3-extra/list?from=e2e"
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://qianchuan.jinritemai.com/star-pages/deep/link?from=e2e"
```

预期结果：

- 每个请求返回 2xx。
- echo 响应中的 `request.parsed_path` 与请求路径一致。
- 请求落到第二条 `127.0.0.1:${ECHO_HTTP_PORT}` 兜底规则，证明第一条长链规则被对应 `excludeFilter` 前缀跳过。
- `/account` 能排除 `/account-center/...`，`/garrmodlistv3` 能排除 `/garrmodlistv3-extra/...`，`/star-pages` 能排除 `/star-pages/...`。

### TC-RFRD-04：Whistle 风格 URL 通配符 filter 跳过当前规则

操作步骤：

1. 启动本地 echo 上游服务。
2. 启动 Bifrost，规则文件包含：

```text
whistle-filter.local 10.37.102.138:8080 excludeFilter://*/api excludeFilter://*/alice/*
whistle-filter.local 127.0.0.1:${ECHO_HTTP_PORT}
```

3. 分别通过 Bifrost 代理访问：

```bash
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://whistle-filter.local/alice/commerce/sale/subscription/entry/config/?from=e2e"
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://whistle-filter.local/prefix/alice/user?from=e2e"
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://whistle-filter.local/api?from=e2e"
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://whistle-filter.local/api/users?from=e2e"
```

预期结果：

- 每个请求返回 2xx。
- echo 响应中的 `request.parsed_path` 与请求路径一致。
- 请求落到第二条 `127.0.0.1:${ECHO_HTTP_PORT}` 兜底规则，证明第一条规则被对应 Whistle wildcard filter 跳过。
- `*/api` 匹配 `/api`、`/api/...` 与 `/api?...`，但不应误匹配 `/apiary`；`*/alice/*` 匹配 URL 中包含 `/alice/` 的请求。

### TC-RFRD-05：单条规则允许不安全上游 HTTPS 证书

操作步骤：

1. 启动本地自签名 HTTPS echo 上游服务。
2. 启动 Bifrost，规则文件包含：

```text
unsafe-upstream.local https://127.0.0.1:${ECHO_HTTPS_PORT} upstreamUnsafeSsl://true
strict-upstream.local https://127.0.0.1:${ECHO_HTTPS_PORT}
```

3. 通过 Bifrost 代理访问：

```bash
curl --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://unsafe-upstream.local/unsafe-cert?from=e2e"
```

预期结果：

- 请求返回 2xx。
- echo 响应中的 `request.parsed_path` 为 `/unsafe-cert`。
- 成功不依赖全局 `--unsafe-ssl`，仅由命中规则的 `upstreamUnsafeSsl://true` 放行自签名上游证书。

### TC-RFRD-06：未配置 `upstreamUnsafeSsl` 的同上游保持严格校验

操作步骤：

1. 复用 TC-RFRD-05 的自签名 HTTPS echo 上游服务和 Bifrost 进程。
2. 通过 Bifrost 代理访问：

```bash
curl -i --proxy "http://127.0.0.1:${PROXY_PORT}" \
  "http://strict-upstream.local/unsafe-cert?from=e2e"
```

预期结果：

- 请求返回 502。
- 响应头包含 `X-Bifrost-Error`。
- 响应正文包含 `upstreamUnsafeSsl://true` 建议，提示用户可以用规则级不安全上游证书放行，而不是启用全局 `--unsafe-ssl`。
- 证明没有 `upstreamUnsafeSsl://true` 的规则仍执行默认安全证书校验。

### TC-RFRD-07：Network `.bifrost` 导出保留实际转发诊断字段

操作步骤：

1. 复用 TC-RFRD-01 产生的 traffic 记录 ID。
2. 调用 network 导出接口：

```bash
curl -X POST \
  -H "Content-Type: application/json" \
  -d "{\"record_ids\":[\"${RECORD_ID}\"],\"include_body\":true}" \
  "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/bifrost-file/export/network"
```

3. 解析 `---` 后的 JSON 内容。

预期结果：

- 第一条记录包含 `has_rule_hit: true`。
- 第一条记录包含 `actual_host: "127.0.0.1"`。
- 第一条记录的 `actual_url` 包含 echo 上游端口。
- 第一条记录的 `listener_port` 等于当前 Bifrost 代理端口。

### TC-RFRD-08：通用并行规则 runner 不重复执行专用回归 fixture

操作步骤：

1. 确认 `e2e-tests/rules/regression/rule_filter_routing_diagnostics.txt` 仅作为专用脚本 fixture 使用。
2. 执行 regression 分类的通用并行规则 runner：

```bash
BIFROST_BIN="$PWD/target/debug/bifrost" \
  e2e-tests/run_all_tests_parallel.sh --no-build -c regression -j 2
```

预期结果：

- 命令返回 0。
- 输出不出现 `rule_filter_routing_diagnostics.txt` 的通用 runner 执行失败。
- 输出不出现 `过滤器规则验证未实现`。
- 专用脚本 `e2e-tests/tests/test_rule_filter_routing_diagnostics.sh` 仍负责执行 TC-RFRD-01 到 TC-RFRD-07 的真实请求断言。

## 清理步骤

1. 停止测试启动的 Bifrost 进程。
2. 停止测试启动的 echo mock 服务。
3. 删除 `.bifrost-e2e-rule-filter-routing-*` 临时数据目录。
