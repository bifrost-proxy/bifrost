# CONNECT 命中非 TLS 改写时自动启用 TLS 拦截

## 背景

Bifrost 的 HTTPS/WSS 客户端在通过 CONNECT / SOCKS5 建立隧道时,如果命中的规则要求把上游改写为明文协议(例如 `https -> http://127.0.0.1:3000` 或 `wss -> ws://127.0.0.1:3001`),必须先在代理与浏览器之间完成 TLS 握手,再把解密后的请求转发到明文上游。否则浏览器会看到 `ERR_SSL_PROTOCOL_ERROR`,SOCKS5 客户端也会因裸 TCP 无法完成 TLS 而失败。

历史实现只在全局开关、`intercept_include`、`app_intercept_include`、`ip_intercept_include` 或规则内 `tlsIntercept://` 显式配置时才启用 MITM。当用户仅写了 host 改写规则,却没有额外声明 `tlsIntercept://`,规则表面上看似合理但实际不生效,导致排查困难。本次是行为修复,不引入新的用户配置,而是把"命中非 TLS 上游改写"作为隐式启用条件之一,同时保留 `tlsPassthrough://` 的最高优先级。

## 用户目标验证清单

### 必须实现

- HTTP CONNECT 隧道命中 `host_protocol == Http | Ws` 的 host 改写规则时,若本地存在 CA 证书,必须自动开启 MITM。
- SOCKS5 TLS 隧道命中同类规则时行为一致,复用 HTTP CONNECT 的决策函数。
- 与全局 `enable_tls_interception`、`intercept_include`、`app_intercept_include`、`ip_intercept_include`、`tlsIntercept://` 一起构成隐式启用集合;任一条件成立即启用。
- `tlsPassthrough://` (即 `resolved_rules.tls_intercept == Some(false)`)必须仍能强制透传,阻止本次自动拦截。

### 必须不破坏

- Host 改写协议为 `Host`、`XHost`、`Https`、`Wss`、`Tunnel`、`Proxy` 时不触发自动拦截,与既有 CONNECT 语义一致。
- 无 CA 证书时(未安装 CA、未启动 bootstrap)不触发自动拦截,维持原透传行为,避免出现无证书 MITM 失败。
- 端口探针 (`force_trust_probe_passthrough`) 与 `!intercept` 冷路径的判定顺序不变。
- 单纯 host 改写到 `Https`/`Wss` 或走 `proxy://` 上游代理仍不需要 MITM。

### 必须真实验证

- 使用真实浏览器 (`chrome --proxy-server=`) 触发 `https://intercept-http.test/` 命中 `host://http://127.0.0.1:3000` 规则,能收到明文上游响应,不再报 `ERR_SSL_PROTOCOL_ERROR`。
- 使用 `curl --socks5 127.0.0.1:port https://intercept-http.test/` 验证 SOCKS5 路径。
- 显式追加 `tlsPassthrough://` 后必须回到透传,浏览器会看到证书直接失败。

## 产品语义

`resolved_rules.host_protocol` 是 rule 解析器写入 `ResolvedRules` 的枚举字段,反映 `host://` 目标协议。当 protocol 为 `Protocol::Http | Protocol::Ws` 时,意味着 CONNECT 请求方是 TLS(浏览器发起 `CONNECT host:443` 之后 ClientHello 上来),但目标是明文 HTTP/WS 上游,只有 MITM 才能把两侧粘起来。

优先级(从高到低):

1. `resolved_rules.tls_intercept == Some(false)` (`tlsPassthrough://`) → 强制透传。
2. 全局 `enable_tls_interception` / include list / `tlsIntercept://` / 客户端 App 白名单 → 强制拦截。
3. 命中非 TLS 上游改写 (`requires_tls_interception_for_host_rewrite`) → 隐式拦截。
4. 其它常规规则 (`requires_tls_interception_for_rules`,例如 res_headers/req_scripts/mock/status_code 等) → 隐式拦截。
5. `has_response_rules_for_host` 或 `has_tls_auto_intercept_route_rules_for_host` → 隐式拦截。

任一 2/3/4/5 命中即开启;1 覆盖所有隐式拦截。

## 技术细节

### 判定函数

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`:

```rust
fn requires_tls_interception_for_host_rewrite(resolved_rules: &ResolvedRules) -> bool {
    resolved_rules.host.is_some()
        && matches!(
            resolved_rules.host_protocol,
            Some(Protocol::Http | Protocol::Ws)
        )
}

pub fn requires_tls_interception_for_connect_rules(resolved_rules: &ResolvedRules) -> bool {
    requires_tls_interception_for_rules(resolved_rules)
        || requires_tls_interception_for_host_rewrite(resolved_rules)
}
```

`should_intercept_tls_for_client` 在 client-only 分支单独调用 `requires_tls_interception_for_host_rewrite`,保证即便 CA 未 build 完 tls_config 但已经有 ca_cert 时也能提前触发。

### HTTP CONNECT 入口

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs` 的 CONNECT MITM 分支在原有 `intercept` 判定后追加:

```rust
if !intercept
    && !force_trust_probe_passthrough
    && tls_config.ca_cert.is_some()
    && !matches!(resolved_rules.tls_intercept, Some(false))
    && (requires_tls_interception_for_connect_rules(&resolved_rules)
        || rules.has_response_rules_for_host(&host)
        || rules.has_tls_auto_intercept_route_rules_for_host(&host))
{
    intercept = true;
}
```

- `tls_config.ca_cert.is_some()` 保证无证书场景不误触发。
- `force_trust_probe_passthrough` 保留证书信任探针路径。
- `tls_intercept == Some(false)` 兜住 `tlsPassthrough://` 逃逸。

### SOCKS5 入口

`crates/bifrost-proxy/src/proxy/socks/tcp.rs` 复用同一判定:

```rust
use crate::proxy::http::tunnel::requires_tls_interception_for_connect_rules;

let do_intercept = crate::proxy::http::should_intercept_tls_for_client(
    /* ... */,
    tls_intercept_config,
    &tls_resolved_rules,
)
    && !matches!(tls_resolved_rules.tls_intercept, Some(false))
    && (requires_tls_interception_for_connect_rules(&tls_resolved_rules)
        || rules.has_tls_auto_intercept_route_rules_for_host(target_host));
```

再在 `relay_with_tls_intercept` 分支进入完整 MITM。

### `host_protocol` 来源

`Protocol::Http | Protocol::Ws` 来自 rule parser。用户如果写 `example.com host://http://127.0.0.1:3000`,parser 把 `http` 解析为 `host_protocol = Protocol::Http`。`https` / `wss` / `host` / `xhost` / `tunnel` 不会触发本次自动拦截。

## CLI + Web + Admin API

- 本次修复不新增 CLI 参数、不新增 Admin API。
- `bifrost status --format json` 中 tls 段已有 `enable_tls_interception`、include list、`ca_cert_present` 字段,可作诊断依据。
- Web UI Traffic 详情页命中规则区块继续展示 `host` 规则内容,MITM 是否发生可通过 traffic record 的 `is_intercepted`/`upstream_scheme` 判断。

## Sync 边界

- 规则本身通过 `RulesStorage` 与 sync 服务同步,无新字段;`host_protocol` 属于运行时解析结果不落 sync payload。
- 本次不改变 `enable_tls_interception` 等 config 字段的 sync 行为。

## Phase 1: 判定函数

- 抽出 `requires_tls_interception_for_host_rewrite`。
- 在 `requires_tls_interception_for_connect_rules` 中合并。
- 在 `should_intercept_tls_for_client` 提前分支调用。
- 补齐相关单元测试。

## Phase 2: HTTP CONNECT 集成

- 修改 CONNECT MITM 决策,追加 host_rewrite 分支。
- 保持 `force_trust_probe_passthrough` 与 `tlsPassthrough://` 优先级。

## Phase 3: SOCKS5 集成

- 修改 `crates/bifrost-proxy/src/proxy/socks/tcp.rs` 的 `do_intercept` 决策,复用 CONNECT 逻辑。
- 与 `tls_resolved_rules.tls_intercept` 组合判定。

## Phase 4: 文档与真实场景验证

- 更新本 design 文档;更新 `human_tests/tls-interception-status-indicators.md` 与其它 tls 用例中对 "只写 host 规则未开 MITM" 的说明。
- 无需 README 变更(纯行为修复)。

## 测试方案

### 单元测试 (`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`)

- `test_requires_tls_interception_for_host_rewrite_only_plaintext_protocols`
- `connect_plaintext_upstream_rewrite_requires_tls_interception`
- `connect_host_rule_alone_does_not_require_tls_interception`
- `connect_proxy_rule_alone_does_not_require_tls_interception`
- `connect_proxy_rule_with_host_rewrite_does_not_use_upstream_proxy`
- `test_requires_tls_interception_for_connect_rules_delegates_to_helpers`
- `test_should_intercept_tls_for_client_respects_rule_override_true_v4`
- `test_should_intercept_tls_for_client_respects_rule_override_false_v4`
- `test_should_intercept_tls_for_client_domain_include_without_app_policy_v4`
- `test_should_intercept_tls_for_client_skips_when_no_ca_cert_v4`

### 集成/E2E 测试

- `e2e-tests/tests/test_tls_intercept_e2e.sh` 覆盖 host 改写触发 MITM。
- `e2e-tests/tests/test_tls_intercept_mode_api.sh` 覆盖显式模式切换与自动拦截共存。
- SOCKS5 相关 `tests/https_proxy_test.rs`, `tests/http_proxy_test.rs`。

### 真实场景

- `human_tests/tls-interception-status-indicators.md`: 触发 `intercept-http.test host://http://127.0.0.1:3000`,浏览器可拿到 `Hello from plaintext upstream`,状态栏保持 `TLS: Scoped` 或 `Off`。
- `human_tests/default-tls-app-whitelist.md`: 无 app 白名单但存在 host 改写,仍能 MITM。
- 使用 `--no-system-proxy`、临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 启动。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核: `resolved_rules.tls_intercept == Some(false)` 是否仍能覆盖所有隐式启用路径。
- 复测: CONNECT 与 SOCKS5 单测、`test_tls_intercept_e2e.sh`、真实浏览器 host 改写。

### 第 2 轮

- 检查 `merge_connect_resolved_rules` 是否可能丢失 `host_protocol`(它已经在合并时同步)。
- 检查 client-only 早期判定分支与后续 CA 就绪时的一致性,避免同一请求出现 "早期决定不拦截、晚期决定拦截" 的分裂。
- 复测: SOCKS5 端到端脚本、`cargo test --workspace --all-features`(交由 CI 或用户手动执行,本地约定 no-local-coverage)。

## 风险与决策

- 规则语义扩展的兼容性: 已有用户如果仅写 `host://http://...` 期待原样透传是不可能的,因为透传下浏览器根本无法完成 TLS。真实的 "希望透传" 场景应改写为 `host://https://...`,行为一致且无回归风险。
- CA 缺失: 自动拦截判定强制要求 `tls_config.ca_cert.is_some()`,避免 "看似启用 MITM 却握手失败" 让排查更困难。
- SOCKS5 与 HTTP CONNECT 共享判定,确保规则一次编写多入口生效;若未来引入新入口(例如 HTTP/3 CONNECT-UDP),仍在同一入口做隐式拦截判定。
- `tlsPassthrough://` 的优先级保留是刻意为之,允许用户对特定域名局部禁用自动拦截。
