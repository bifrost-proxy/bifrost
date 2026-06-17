# Rule TLS Intercept Routing Exceptions

## 功能模块说明

Bifrost 的规则引擎在全局 TLS 解包关闭时，仍会因为某些规则命中而自动开启 TLS 解包。这个自动开启只应该用于必须读取或修改 TLS 内层 HTTP 内容的规则，例如请求头、响应头、body、脚本、mock、状态码等。

本模块要求补齐以下自动解包边界：

- 带具体域名/IP 作用域的路由规则应自动开启 TLS 解包，即使匹配器没有 `https://` 协议前缀。CONNECT/SOCKS5 阶段只能看到 host，必须先解包才能让内层 HTTPS path 参与规则优先级匹配。
- 仅命中 `proxy://` 的下游代理规则不应自动开启 TLS 解包。它只决定把流量交给另一个代理，不需要解析 TLS 内层 HTTP。
- `proxy://` 是严格例外：即使 matcher 写成具体域名或路径（例如 `example.com/app proxy://downstream:8080`），只要目标协议只有下游代理转发这一类，就不能因为这条 proxy 规则自动 TLS 解包。
- 规则驱动的自动 TLS 解包必须有明确 host 作用域：Domain、IP/CIDR、带具体域名/IP 片段的 Wildcard/PathWildcard 可以触发；纯 Regex、裸 `*`、`*/path/*` 这类没有域名/IP 片段的纯通配规则不能触发。

## 当前现状

代码检查结论：

- HTTP CONNECT 入口 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 已经把内容改写类规则、`http://` / `ws://` 明文上游 rewrite、显式 `tlsIntercept://`、include/global TLS 配置作为解包理由。本次新增具体 host 作用域路由规则的自动解包判断，覆盖 `example.com/path https://upstream` 这类无协议前缀配置。
- SOCKS5 TLS 入口 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 与 HTTP CONNECT 共用具体 host 作用域路由判断，避免两个入口对同一规则产生不同 TLS 解包结果。
- `proxy://` 当前没有被该 SOCKS5 独立判断直接作为解包理由，但需要单元和 E2E 固化，避免后续把下游代理规则纳入自动解包。
- `RulesResolver::has_response_rules_for_host` 与 `RulesResolver::has_tls_auto_intercept_route_rules_for_host` 都必须区分 matcher 是否具备明确 host 作用域；纯 regex 或裸 wildcard 不能成为自动解包理由。

## 实现逻辑

1. 在 matcher trait 增加 host 作用域粗匹配：
   - `matches_host_scope(url, host)` 默认复用 `matches_host`。
   - Domain/IP 保持既有 host 匹配。
   - Wildcard/PathWildcard 在自动 TLS 判断中剥离路径后比较 matcher 的具体 host 作用域，避免 CONNECT 阶段因为没有 path 而漏掉 `example.com/path`。
2. 在 `RulesResolver` 增加 `has_tls_auto_intercept_route_rules_for_host`：
- 只纳入 `host://`、`xhost://`、`http://`、`https://`、`ws://`、`wss://` 这类目标路由协议；matcher 不要求带协议前缀，只要具备具体 host 作用域并覆盖当前 TLS host。
   - 要求 matcher 可触发自动 TLS 解包，并且 matcher 的 host 作用域覆盖当前 CONNECT/SOCKS5 host。
   - 严格不纳入 `proxy://`，避免下游代理选择本身强制解包；即使 proxy 规则带路径也保持这个边界。
3. 在 HTTP tunnel 模块保留统一 helper：
   - `requires_tls_interception_for_connect_rules`
   - 语义：内容读写规则需要解包，或 HTTPS/WSS CONNECT 被规则改写到明文 `http://` / `ws://` 上游时需要解包。
4. HTTP CONNECT 自动解包分支使用“内容规则 / 明文上游 rewrite / 明确 host 路由规则 / 响应规则”四类理由。
5. HTTP CONNECT 与 SOCKS5 TCP 命中纯 `proxy://` 时，通过下游 HTTP proxy 发送 `CONNECT original_host:port` 后再透传原始 TLS 字节，确保下游代理真实承载流量。
6. SOCKS5 TLS 自动解包分支使用与 HTTP CONNECT 相同的 host 作用域路由判断。
7. 在 matcher trait 保留 `can_trigger_tls_auto_intercept`：
   - `DomainMatcher`、`IpMatcher` 可作为自动解包依据。
   - `WildcardMatcher`、`PathWildcardMatcher` 只有 pattern 的 host 部分包含具体域名/IP 片段时可作为自动解包依据，例如 `*.example.com`、`^api.example.com/v1/*`。
   - `RegexMatcher` 以及纯 `*`、`*/api/*` 等无 host 片段通配规则不能作为自动解包依据。
8. 保留显式优先级：
   - `tlsIntercept://` 仍强制解包。
   - `tlsPassthrough://` 仍强制透传。
   - 全局 TLS、域名 include、App include 仍可触发解包。

## 依赖项

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
- `crates/bifrost-proxy/src/proxy/socks/tcp.rs`
- `crates/bifrost-proxy/src/proxy/http/handler.rs`（`connect_via_upstream_http_proxy_tunnel` 承担 proxy-only 命中后的下游 HTTP CONNECT 转发）
- `crates/bifrost-core/src/matcher/*`
- `crates/bifrost-core/src/rule/resolver.rs`
- `crates/bifrost-cli/src/parsing/rules.rs`（CLI 侧 `RulesResolver` 包装实现，需保持与 core trait 一致）
- `e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`
- `e2e-tests/rules/socks5_tls/routing_exceptions.txt`
- `e2e-tests/run_all_tests_parallel.sh`（`FIXTURE_ONLY_RULES` 必须包含 `socks5_tls/routing_exceptions.txt`）

## 测试方案

### 单元测试

- `connect_host_rule_alone_does_not_require_tls_interception`
- `connect_proxy_rule_alone_does_not_require_tls_interception`
- `connect_plaintext_upstream_rewrite_requires_tls_interception`
- `connect_content_mutation_requires_tls_interception_even_with_proxy_rule`
- `test_has_tls_auto_intercept_route_rules_for_host_allows_plain_domain_routes`
- `test_has_tls_auto_intercept_route_rules_for_host_respects_scheme_scope`
- `test_has_tls_auto_intercept_route_rules_for_host_rejects_proxy_only_and_broad_patterns`
- `test_has_response_rules_for_host_allows_explicit_domain_and_ip_scope`
- `test_has_response_rules_for_host_allows_host_scoped_wildcards`
- `test_has_response_rules_for_host_rejects_pure_regex_and_wildcards`

### E2E 测试

新增 SOCKS5 专项脚本：

- `TC-S5TRE-01`: 全局 TLS 关闭，`proxy://` 命中，上游 SOCKS5 Bifrost 把 HTTPS CONNECT 转发给下游 Bifrost HTTP proxy；请求成功，上游日志和 Traffic 不出现 HTTPS 解包记录，下游 Traffic 出现目标 CONNECT 记录。
- `TC-S5TRE-02`: 全局 TLS 关闭，`proxy://` 命中，上游 HTTP CONNECT Bifrost 把 HTTPS CONNECT 转发给下游 Bifrost HTTP proxy；请求成功，上游日志和 Traffic 不出现 HTTPS 解包记录，下游 Traffic 出现目标 CONNECT 记录。
- `TC-S5TRE-03`: 全局 TLS 关闭，裸 `* resHeaders://...` 命中，但因缺少 host 作用域不自动解包，响应头不被应用。
- `TC-S5TRE-04`: 全局 TLS 关闭，纯 regex `resHeaders://...` 命中，但不自动解包，响应头不被应用。
- `TC-S5TRE-05`: 全局 TLS 关闭，明确域名 `resHeaders://` 命中，HTTPS 通过 SOCKS5 自动解包并应用响应头，证明内容改写类规则没有退化。
- `TC-S5TRE-06`: 全局 TLS 关闭，`domain-path-auto.local/app/account-center https://...` 这类无协议前缀的具体域名路径路由命中后自动解包，并按内层 path 选择最具体上游。
- `TC-S5TRE-07`: `routing_exceptions.txt` 是跨规则语义 fixture，必须由专项脚本执行；并行通用 rules runner 应把它列入 `FIXTURE_ONLY_RULES`，避免把 `host://` / `resHeaders://` 混合语义误套普通单规则断言。当前由 `e2e-tests/run_all_tests_parallel.sh` 的 `FIXTURE_ONLY_RULES` 数组承担（脚本中没有同名 case，仅作隔离约束）；`test_socks5_tls_routing_exceptions.sh` 内只实现 TC-S5TRE-01..06。

### 真实场景测试

新增 `human_tests/rule-tls-intercept-routing-exceptions.md`，按 E2E 同等场景真实执行 CLI/API 验证，并同步 `human_tests/readme.md` 索引。当前 human_tests 文档覆盖 TC-S5TRE-01..06（与 E2E 脚本对齐），并扩展了两个仅在真实场景执行的用例：TC-S5TRE-07（用户反馈导出的 `qianchuan.jinritemai.com/app/account-center` 路径优先级回归，配套 `cargo test -p bifrost-admin test_passthrough_does_not_override_higher_priority_forward_rule`）和 TC-S5TRE-08（验证 `routing_exceptions.txt` 被 `run_all_tests_parallel.sh` 的 `FIXTURE_ONLY_RULES` 隔离）。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：无协议前缀的具体域名路径路由自动 TLS 解包并命中最具体规则，proxy-only 不自动 TLS 解包且真实经过下游代理，纯 regex/wildcard 不自动 TLS 解包，明确 host/IP 内容改写仍自动解包。
- 复核 diff：`git status --short`、`git diff`。
- 运行聚焦测试：`cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`、新增 E2E、human_tests。
- 修复发现的问题并复跑失败路径。

### 第 2 轮

- 复查第 1 轮后最新 diff，确认 HTTP CONNECT 与 SOCKS5 入口语义一致。
- 复查 design、用户手册、E2E fixture、human_tests/readme.md 是否同步。
- 复跑聚焦单元和 E2E。
- 若仍发现功能缺陷或测试缺口，追加第 3 轮。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-core has_response_rules_for_host --all-features`
- `cargo test -p bifrost-core has_tls_auto_intercept_route_rules_for_host --all-features`
- `cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`
- `BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`
- `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 bash e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once`
- `cargo test --workspace --all-features`
- 按修改范围决定是否执行 `scripts/ci/local-ci.sh`；若不执行，需要在最终验证矩阵说明原因和风险。

## 文档更新要求

本次不新增协议、不改 CLI 参数、不改配置默认值，因此 README 协议/CLI 文档不需要更新。必须更新：

- `design/rule-tls-intercept-routing-exceptions.md`
- `docs/rule.md`
- `docs/rules/routing.md`
- `human_tests/rule-tls-intercept-routing-exceptions.md`
- `human_tests/readme.md`
