# Rule TLS Intercept Routing Exceptions

## 背景

Bifrost 的规则引擎在全局 TLS 解包（`tls_intercept_enabled`）关闭时，仍会因为某些规则命中而自动开启 TLS 解包。这个自动开启只应该用于必须读取或修改 TLS 内层 HTTP 内容的规则——请求头、响应头、body、脚本、mock、status。对于纯路由类规则（`host://`、`http://` 明文改写等），当 matcher 带有具体 host 作用域时也需要自动解包，因为 CONNECT/SOCKS5 阶段只能看到 host，必须先解包内层 HTTPS path 才能让最具体的规则优先命中。

真实用户反馈（`qianchuan.jinritemai.com/app/account-center`）暴露过：一条带具体 host 作用域的 `passthrough://` / `tlsPassthrough://` 规则会覆盖一条更具体的转发规则，导致原本应命中的 `host://` 上游被路由到透传。修复要求：`proxy://` 是严格例外，即使 matcher 具体到域名或路径，也不能因为它自动 TLS 解包——`proxy://` 只把流量交给下游代理，不需要读 TLS 内层内容。

本模块要求补齐以下自动解包边界：

- 带具体域名/IP 作用域的路由规则应自动开启 TLS 解包，即使匹配器没有 `https://` 协议前缀。
- 仅命中 `proxy://` 的下游代理规则不应自动开启 TLS 解包。
- `proxy://` 是严格例外：即使 matcher 写成具体域名或路径（例如 `example.com/app proxy://downstream:8080`），只要目标协议只有下游代理转发一类，就不能自动 TLS 解包。
- 规则驱动的自动 TLS 解包必须有明确 host 作用域：Domain、IP/CIDR、带具体域名/IP 片段的 Wildcard/PathWildcard 可以触发；纯 Regex、裸 `*`、`*/path/*` 这类无域名/IP 片段的纯通配规则不能触发。

## 用户目标验证清单

### 必须实现

- 具体 host 作用域路由规则（无协议前缀）自动 TLS 解包，并按内层 path 选择最具体上游。
- `proxy://` 命中时上游 Bifrost（HTTP CONNECT / SOCKS5）通过 `CONNECT original_host:port` 把原始 TLS 字节直接交给下游 HTTP proxy，全程不解包。
- 纯 `*` / `*/api/*` / 纯 regex 规则不触发自动 TLS 解包。
- `tlsIntercept://` 强制解包；`tlsPassthrough://` 强制透传；两者不被上述规则驱动逻辑覆盖。
- 全局 TLS、域名 include、App include 仍可触发解包。
- HTTP CONNECT 与 SOCKS5 TLS 入口对同一规则集产生一致的 TLS 解包结果。

### 必须不破坏

- 内容改写类规则（`reqHeaders://` / `resHeaders://` / `resBody://` / `resStatus://` / `script://` / `mock://`）在具体 host 作用域下继续自动解包。
- HTTPS/WSS CONNECT 被规则改写到明文 `http://` / `ws://` 上游时继续解包。
- Rule specificity（更具体 matcher 优先）保持不变；被误覆盖的用户上游规则不再被 `passthrough://` / `tlsPassthrough://` 吃掉。
- Traffic 日志仍能标记命中的 rule source。

### 必须真实验证

- 真实 SOCKS5 + `proxy://` 命中：上游 Bifrost 不出现 HTTPS 解包记录，下游 Bifrost 出现目标 CONNECT。
- 真实 HTTP CONNECT + `proxy://` 命中：同上。
- 真实 `example.com/path https://upstream` 无协议前缀路由：自动解包并命中最具体规则。
- 真实纯通配 `* resHeaders://...`：不触发解包，响应头不被应用。
- 真实明确域名 `resHeaders://`：自动解包并应用响应头（内容改写没有退化）。

## 产品语义

### 自动 TLS 解包判断的四类理由（HTTP CONNECT）

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs::requires_tls_interception_for_connect_rules` 汇总四类理由：

1. 内容读写类规则（`has_response_rules_for_host` / `has_content_mutation_rules_for_host`）。
2. HTTPS/WSS CONNECT 被规则改写到明文 `http://` / `ws://` 上游。
3. 明确 host 路由规则（`has_tls_auto_intercept_route_rules_for_host`）——目标协议是 `host://` / `xhost://` / `http://` / `https://` / `ws://` / `wss://`，且 matcher 具备具体 host 作用域并覆盖当前 CONNECT host。
4. `tlsIntercept://` 强制解包 / include 白名单命中。

SOCKS5 TLS 入口 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 复用同一 helper，两者对同一规则产生一致结果。

### `proxy://` 严格例外

- `has_tls_auto_intercept_route_rules_for_host` 明确不纳入 `proxy://`。
- 即便 matcher 是 `example.com/app proxy://downstream:8080`，也不因“具体 host 作用域”而解包。
- CONNECT 阶段命中纯 `proxy://` 时通过 `connect_via_upstream_http_proxy_tunnel` 建立到下游 HTTP proxy 的 `CONNECT original_host:port`，然后透传原始 TLS。
- 内容改写与 `proxy://` 同时命中时，内容改写理由仍会触发解包；`proxy://` 只在“唯一命中协议”时保持透传。

### matcher 的 host 作用域

`crates/bifrost-core/src/matcher/*` 中新增 `matches_host_scope(url, host)` 与 `can_trigger_tls_auto_intercept`：

- `DomainMatcher` / `IpMatcher`：可作为自动解包依据。
- `WildcardMatcher` / `PathWildcardMatcher`：仅当 pattern 的 host 部分包含具体域名/IP 片段（如 `*.example.com`、`^api.example.com/v1/*`）才可以作为自动解包依据。
- `RegexMatcher` 与纯 `*` / `*/api/*` 通配规则不能作为自动解包依据。
- CONNECT 阶段 matcher 剥离路径后比较 host 作用域，避免因为没有 path 而漏掉 `example.com/path`。

### 显式优先级

- `tlsIntercept://` 仍强制解包。
- `tlsPassthrough://` 仍强制透传（对应 admin 测试 `test_passthrough_does_not_override_higher_priority_forward_rule` 保证更具体的转发规则仍能胜出）。
- 全局 TLS、域名 include、App include 仍可触发解包。

## 技术细节

### resolver 层能力

- `RulesResolver::has_tls_auto_intercept_route_rules_for_host` 返回 bool，覆盖点：`host://`、`xhost://`、`http://`、`https://`、`ws://`、`wss://`；严格排除 `proxy://`；matcher 必须 `can_trigger_tls_auto_intercept` + `matches_host_scope(host)`。
- `RulesResolver::has_response_rules_for_host` 与 `has_content_mutation_rules_for_host`：只把带明确 host/IP 作用域或 host-scoped wildcard 的内容规则纳入；纯 regex/wildcard 不算。

### HTTP CONNECT 入口

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`：`decide_tls_action_for_connect` 依次检查 include、`tlsIntercept://`、`tlsPassthrough://`、`requires_tls_interception_for_connect_rules`（四类理由）；仅 `proxy://` 命中时走 `connect_via_upstream_http_proxy_tunnel`。
- `crates/bifrost-proxy/src/proxy/http/handler.rs::connect_via_upstream_http_proxy_tunnel`：与下游 HTTP proxy 建立 `CONNECT original_host:port`，成功后透传原始 TLS。

### SOCKS5 入口

- `crates/bifrost-proxy/src/proxy/socks/tcp.rs`：TLS 前先构造 `RuleSelectionContext`，走同一 `requires_tls_interception_for_connect_rules` 判断；`proxy://` 命中走同一下游 HTTP proxy tunnel 路径。

### CLI 侧 resolver wrapper

- `crates/bifrost-cli/src/parsing/rules.rs` 中的 `RulesResolver` 包装必须与 core trait 保持一致，避免 CLI-parsed 规则产生不同的自动解包结果。

## CLI + Web + Admin API

本次改动不新增协议、CLI 参数或 admin API；语义变化只影响运行时：

- `bifrost rule list` / `bifrost rule show` 保持不变。
- `bifrost port active <port>` 展示的命中规则来源保持不变。
- Web UI Rules 编辑器不需要新增控件；文档内需说明纯通配规则不会自动 TLS 解包。
- Admin API `POST /api/rules/validate` 不承担运行时解包判断；语法层不阻止用户写纯通配 `resHeaders://` 规则，但用户需了解运行时无法生效。

## Sync 边界

- 语义只影响运行时决策，不影响规则文件内容，也不影响 Sync 序列化。
- 已同步的规则若之前依赖 “纯通配 `resHeaders://` 自动解包” 语义，可能在升级后失效；文档需在 `docs/rule.md` 与 `docs/rules/routing.md` 明确说明。

## Phase 划分

### Phase 1：matcher 与 resolver 能力

- 在 matcher trait 增加 `matches_host_scope` / `can_trigger_tls_auto_intercept`。
- 在 `RulesResolver` 增加 `has_tls_auto_intercept_route_rules_for_host`，调整 `has_response_rules_for_host` / `has_content_mutation_rules_for_host` 的作用域判断。
- 单元测试全覆盖。

### Phase 2：HTTP CONNECT 入口整合

- 统一 helper `requires_tls_interception_for_connect_rules`。
- 内容 / 明文上游 rewrite / 明确 host 路由 / 响应四类理由。
- `proxy://` 走 `connect_via_upstream_http_proxy_tunnel`。

### Phase 3：SOCKS5 TLS 入口对齐

- SOCKS5 TCP handler 复用同一 helper。
- `proxy://` 命中走下游 HTTP proxy tunnel。

### Phase 4：文档与真实场景验证

- 更新 `design/rule-tls-intercept-routing-exceptions.md`、`docs/rule.md`、`docs/rules/routing.md`、`human_tests/rule-tls-intercept-routing-exceptions.md`、`human_tests/readme.md`。
- 补齐 e2e-tests fixture 与并行 runner 隔离（`FIXTURE_ONLY_RULES`）。

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
- Admin 侧 `test_passthrough_does_not_override_higher_priority_forward_rule`（保证 `tlsPassthrough://` 不吃掉更具体的转发规则）。

### E2E 测试

`e2e-tests/tests/test_socks5_tls_routing_exceptions.sh` + `e2e-tests/rules/socks5_tls/routing_exceptions.txt`，跑真实 SOCKS5 + HTTP CONNECT 双入口，覆盖 TC-S5TRE-01..06：

- `TC-S5TRE-01`：全局 TLS 关，`proxy://` 命中，上游 SOCKS5 Bifrost 把 HTTPS CONNECT 转发给下游 HTTP proxy；上游 Traffic 无 HTTPS 解包记录，下游 Traffic 有目标 CONNECT。
- `TC-S5TRE-02`：全局 TLS 关，`proxy://` 命中，上游 HTTP CONNECT Bifrost 转发给下游 HTTP proxy；同上。
- `TC-S5TRE-03`：全局 TLS 关，裸 `* resHeaders://...` 命中，但缺 host 作用域不解包，响应头不被应用。
- `TC-S5TRE-04`：全局 TLS 关，纯 regex `resHeaders://...` 命中，同上。
- `TC-S5TRE-05`：全局 TLS 关，明确域名 `resHeaders://` 命中，HTTPS 通过 SOCKS5 自动解包并应用响应头，证明内容改写没有退化。
- `TC-S5TRE-06`：全局 TLS 关，`domain-path-auto.local/app/account-center https://...` 无协议前缀具体域名路径路由命中后自动解包，并按内层 path 选择最具体上游。

`e2e-tests/run_all_tests_parallel.sh` 的 `FIXTURE_ONLY_RULES` 必须包含 `socks5_tls/routing_exceptions.txt`，把它排除在并行通用 rules runner 之外——它是跨规则语义 fixture，普通单规则断言不适用。

### human_tests

`human_tests/rule-tls-intercept-routing-exceptions.md` 覆盖 TC-S5TRE-01..06（与 E2E 对齐），另外扩展仅在真实场景执行的两个用例：

- `TC-S5TRE-07`：用户反馈 `qianchuan.jinritemai.com/app/account-center` 路径优先级回归，配套 `cargo test -p bifrost-admin test_passthrough_does_not_override_higher_priority_forward_rule`。
- `TC-S5TRE-08`：验证 `routing_exceptions.txt` 被 `run_all_tests_parallel.sh` 的 `FIXTURE_ONLY_RULES` 隔离。

`human_tests/readme.md` 索引同步更新。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：无协议前缀具体域名路径路由自动 TLS 解包并命中最具体规则；proxy-only 不自动解包且真实经过下游代理；纯 regex/wildcard 不自动解包；明确 host/IP 内容改写仍自动解包。
- 复核 diff：`git status --short`、`git diff`。
- 运行聚焦测试：`cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`；新增 E2E；human_tests。

### 第 2 轮

- 复查 HTTP CONNECT 与 SOCKS5 入口语义一致。
- 复查 design、用户手册、E2E fixture、human_tests/readme.md。
- 复跑聚焦单元和 E2E。若仍缺口追加第 3 轮。

## 校验命令

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-core has_response_rules_for_host --all-features`
- `cargo test -p bifrost-core has_tls_auto_intercept_route_rules_for_host --all-features`
- `cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`
- `cargo test -p bifrost-admin test_passthrough_does_not_override_higher_priority_forward_rule --all-features`
- `BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`
- `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 bash e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once`
- `cargo test --workspace --all-features`
- 按修改范围决定是否执行 `scripts/ci/local-ci.sh`；若不执行，最终验证矩阵说明原因和风险。

## 文档更新要求

不新增协议、不改 CLI 参数、不改配置默认值，因此 README 协议/CLI 文档不需要更新。必须更新：

- `design/rule-tls-intercept-routing-exceptions.md`
- `docs/rule.md`
- `docs/rules/routing.md`
- `human_tests/rule-tls-intercept-routing-exceptions.md`
- `human_tests/readme.md`

## 风险与决策

- **`passthrough://` 与更具体转发规则冲突**：admin `test_passthrough_does_not_override_higher_priority_forward_rule` 已作为回归护栏；若未来引入新的 passthrough 类协议，必须补同类断言。
- **纯通配 `resHeaders://` 语义变化**：老规则若依赖 `* resHeaders://...` 自动解包，会在升级后失效；文档必须显式说明，避免用户误以为规则未生效是 bug。
- **`proxy://` + 内容改写混合命中**：目前实现让内容改写理由触发解包，符合“需要读内层内容才能改写”的直觉；若产品未来希望完全尊重 `proxy://` 不解包，必须为混合命中显式定义优先级。
- **CLI resolver 与 core resolver 漂移风险**：`crates/bifrost-cli/src/parsing/rules.rs` 中的 wrapper 必须持续与 core trait 对齐；引入新的 matcher 类型时要同步更新两个地方，否则 CLI 打印的“会自动解包”与运行时行为不一致。
