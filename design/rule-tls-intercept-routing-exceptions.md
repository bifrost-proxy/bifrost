# Rule TLS Intercept Routing Exceptions

## 功能模块说明

Bifrost 的规则引擎在全局 TLS 解包关闭时，仍会因为某些规则命中而自动开启 TLS 解包。这个自动开启只应该用于必须读取或修改 TLS 内层 HTTP 内容的规则，例如请求头、响应头、body、脚本、mock、状态码等。

本次需求要求补齐两个例外：

- 仅命中 `host://` 的路由规则不应自动开启 TLS 解包。它只改变 CONNECT 或 SOCKS5 目标地址，不需要读取 TLS 内层 HTTP。
- 仅命中 `proxy://` 的下游代理规则不应自动开启 TLS 解包。它只决定把流量交给另一个代理，不需要解析 TLS 内层 HTTP。

## 当前现状

代码检查结论：

- HTTP CONNECT 入口 `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 已经只把内容改写类规则、`http://` / `ws://` 明文上游 rewrite、显式 `tlsIntercept://`、include/global TLS 配置作为解包理由。`host://` 和 `proxy://` 不在 `requires_tls_interception_for_rules` 中。
- SOCKS5 TLS 入口 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 存在独立判断：`tls_resolved_rules.host.is_some() && !tls_resolved_rules.ignored.host` 会让任意 host 命中触发 TLS 解包。这会把纯 `host://` 路由误判为需要解包。
- `proxy://` 当前没有被该 SOCKS5 独立判断直接作为解包理由，但需要单元和 E2E 固化，避免后续把下游代理规则纳入自动解包。

## 实现逻辑

1. 在 HTTP tunnel 模块新增统一 helper：
   - `requires_tls_interception_for_connect_rules`
   - 语义：内容读写规则需要解包，或 HTTPS/WSS CONNECT 被规则改写到明文 `http://` / `ws://` 上游时需要解包。
2. HTTP CONNECT 自动解包分支改用该 helper，保持现有行为。
3. SOCKS5 TLS 自动解包分支改用该 helper，删除“任意 host 命中都解包”的分叉逻辑。
4. 保留显式优先级：
   - `tlsIntercept://` 仍强制解包。
   - `tlsPassthrough://` 仍强制透传。
   - 全局 TLS、域名 include、App include 仍可触发解包。

## 依赖项

- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
- `crates/bifrost-proxy/src/proxy/socks/tcp.rs`
- `e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`
- `e2e-tests/rules/socks5_tls/routing_exceptions.txt`

## 测试方案

### 单元测试

- `connect_host_rule_alone_does_not_require_tls_interception`
- `connect_proxy_rule_alone_does_not_require_tls_interception`
- `connect_plaintext_upstream_rewrite_requires_tls_interception`
- `connect_content_mutation_requires_tls_interception_even_with_proxy_rule`

### E2E 测试

新增 SOCKS5 专项脚本：

- `TC-S5TRE-01`: 全局 TLS 关闭，`host://` 命中，HTTPS 通过 SOCKS5 请求成功，日志和 Traffic 不出现 HTTPS 解包记录。
- `TC-S5TRE-02`: 全局 TLS 关闭，`proxy://` 命中，HTTPS 通过 SOCKS5 请求成功，日志和 Traffic 不出现 HTTPS 解包记录。
- `TC-S5TRE-03`: 全局 TLS 关闭，`resHeaders://` 命中，HTTPS 通过 SOCKS5 自动解包并应用响应头，证明内容改写类规则没有退化。
- `TC-S5TRE-04`: `routing_exceptions.txt` 是跨规则语义 fixture，必须由专项脚本执行；并行通用 rules runner 应把它列入 `FIXTURE_ONLY_RULES`，避免把 `host://` / `resHeaders://` 混合语义误套普通单规则断言。

### 真实场景测试

新增 `human_tests/rule-tls-intercept-routing-exceptions.md`，按 E2E 同等场景真实执行 CLI/API 验证，并同步 `human_tests/readme.md` 索引。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：host-only / proxy-only 不自动 TLS 解包，内容改写仍自动解包。
- 复核 diff：`git status --short`、`git diff`。
- 运行聚焦测试：`cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`、新增 E2E、human_tests。
- 修复发现的问题并复跑失败路径。

### 第 2 轮

- 复查第 1 轮后最新 diff，确认 HTTP CONNECT 与 SOCKS5 入口语义一致。
- 复查 design、E2E fixture、human_tests/readme.md 是否同步。
- 复跑聚焦单元和 E2E。
- 若仍发现功能缺陷或测试缺口，追加第 3 轮。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-proxy requires_tls_interception_for_connect_rules --all-features`
- `BIFROST_BIN=./target/release/bifrost bash e2e-tests/tests/test_socks5_tls_routing_exceptions.sh`
- `BIFROST_E2E_RULE_JOBS=1 BIFROST_E2E_RETRY_FAILED_ONCE=1 bash e2e-tests/run_all_tests_parallel.sh -c socks5_tls --no-build --retry-failed-once`
- `cargo test --workspace --all-features`
- 按修改范围决定是否执行 `scripts/ci/local-ci.sh`；若不执行，需要在最终验证矩阵说明原因和风险。

## 文档更新要求

本次不新增协议、不改 CLI 参数、不改配置默认值，因此 README 协议/CLI 文档不需要更新。必须更新：

- `design/rule-tls-intercept-routing-exceptions.md`
- `human_tests/rule-tls-intercept-routing-exceptions.md`
- `human_tests/readme.md`
