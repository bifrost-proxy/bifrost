# host 与 proxy 规则优先级修正方案

## 背景

Bifrost 规则语言允许在同一行或多条规则里同时命中多个协议。`host://` 用于把请求重写到另一个直连上游，`proxy://` 则用于把请求转发到指定的上游 HTTP/SOCKS 代理。用户在写规则时常常会“host 命中就直连该地址、不再走 proxy”，例如：

```text
api.corp.example status://200
api.corp.example host://10.0.0.53
api.corp.example proxy://[ph_IP_ADDRESS_2_ph]:8888
```

期望语义：请求 `https://api.corp.example/*` 直接命中 host 重写到 `10.0.0.53`，proxy 只在没有 host 命中时兜底。但历史实现里，HTTP 上游发送、CONNECT 隧道、SOCKS TCP 三条链路的“是否使用上游代理”判定并不统一：HTTP 直接看 `resolved_rules.proxy.is_some()`，忽略了 host 是否命中；tunnel 与 SOCKS 各自拷贝了近似逻辑但没考虑 host ignore 标记；HTTP/3 预判又把 proxy 当作 hard disable，导致 host 命中时反而被 fallback 到不受期望的路径。

社区反馈以及本仓库 E2E `test_routing_host_vs_proxy` / `test_priority_host_vs_proxy` 一直在期望“host 优先，命中 host 就直连”，历史实现让这两条用例长期红着。本文档描述把该行为收敛到一个统一判定函数上，并对 CONNECT、SOCKS、HTTP/3 预判全部套用同一语义。

## 用户目标验证清单

### 必须实现

- 同一请求同时命中 `host://` 与 `proxy://` 时，只要 host 有效，就走 host 目标直连，不再送到上游代理。
- host 被规则显式忽略（例如通过 `@ignore host` 或 `resolved_rules.ignored.host = true`）时，proxy 继续生效。
- host 不存在或未命中时，proxy 保留原有兜底作用。
- 逻辑对普通 HTTP forwarding、HTTPS CONNECT 隧道、SOCKS TCP、HTTP/3 预判全部一致。
- 命中详情（`matched_rules`、traffic detail）仍展示两个协议都被匹配到，方便排查。

### 必须不破坏

- 只命中 `proxy://` 的请求继续走上游代理。
- 只命中 `host://` 的请求继续走 host 目标。
- SOCKS UDP、HTTP/3 QUIC 建连、PAC/system proxy 兜底、CONNECT 上行认证、proxy chain auth 等既有行为不变。
- `@ignore proxy`、`@ignore host` 语义保持一致。
- Traffic 展示的 `matched_rules` 和 `used_upstream_proxy` 字段仍准确。

### 必须真实验证

- E2E `test_routing_host_vs_proxy` 与 `test_priority_host_vs_proxy` 必须恢复通过，且不再出现 502 重试。
- SOCKS TCP 场景通过真实 curl+SOCKS client 验证 host 优先。
- HTTP/3 预判需要通过日志或诊断字段证明 host 命中不会被 proxy 禁用 h3。

## 产品语义

### 单点判定函数

引入或复用统一入口：

```rust
pub fn should_use_upstream_proxy(rr: &ResolvedRules) -> bool {
    if rr.proxy.is_none() {
        return false;
    }
    let host_active = rr.host.is_some() && !rr.ignored.host;
    !host_active
}
```

- 只有 proxy 存在且 host 未生效时才返回 true。
- host 存在但被 `ignored.host` 标记时视为未生效，proxy 继续用。
- 若同时存在 proxy 与 host 且都未被忽略，返回 false（host 胜出）。

CONNECT 与 SOCKS 侧的对应函数分别为：

```rust
pub fn should_use_connect_upstream_proxy(rr: &ResolvedRules) -> bool;
pub fn should_use_socks_upstream_proxy(rr: &ResolvedRules) -> bool;
```

三者共享一个内部 helper，仅在协议特化差异（例如 SOCKS 不支持某些 CONNECT 认证方案）时才走各自适配。

### 命中详情保留

`matched_rules` 与 `matched_protocols` 仍展示 host 和 proxy 都命中，让用户能在 traffic detail 里看到“规则里其实两个都写了”。新增字段 `applied_upstream = "host_direct" | "proxy_chain" | "direct"` 表达最终实际行为，便于排查“为什么明明写了 proxy 却没走代理”。

### HTTP/3 与 QUIC

HTTP/3 预判之前用“存在 proxy 就禁用 h3”，会误把 host 优先的场景也降级。新语义：只有 `should_use_upstream_proxy` 返回 true 时才禁用 h3；host 优先场景仍允许 h3 直连上游。

## 技术细节

### 修改点

- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - `should_use_upstream_proxy` 从散落分支收敛到 helper 内。
  - h3 预判改为调用同一 helper。
  - 请求构造阶段读取 `applied_upstream` 用于 traffic record。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - `should_use_connect_upstream_proxy` 复用 helper。
  - CONNECT payload/authority 组装保留 host 直连时对目标 host:port 的解析，不再往 proxy tunnel 写入。
- `crates/bifrost-proxy/src/proxy/socks/tcp.rs`
  - `should_use_socks_upstream_proxy` 复用 helper。
  - SOCKS reply 中的最终 bind address 使用 host 目标。
- `crates/bifrost-e2e/src/tests/routing.rs`、`rule_priority.rs`
  - 补齐 host+proxy 同时命中场景的断言。
- `crates/bifrost-proxy/src/traffic/record.rs`
  - 新增 `applied_upstream` 字段。

### 日志与诊断

- host 与 proxy 都命中时打印 `debug! target: "bifrost::proxy::route", "host wins over proxy: host={host} proxy={proxy}"`。
- 若 `ignored.host = true` 且 proxy 生效，打印 `debug!` 说明 host 被忽略。
- Traffic detail 里 `applied_upstream` 字段以 chip 展示，前端 rules panel tooltip 补充说明。

## CLI/Web/Admin API

### CLI

- `bifrost traffic get <id> --format json` 输出新增 `applied_upstream` 字段。
- `bifrost rule check <file>` 若发现同一 pattern 同时写 host 与 proxy，给出提示 `note: host wins over proxy in current precedence`。

### Web

- Traffic detail 的 Rules 面板增加 `Applied upstream: host_direct` 展示；host 命中时 proxy 行用灰色标记 `overridden by host`。

### Admin API

- `GET /api/traffic/:id` 响应新增 `applied_upstream`。
- `GET /api/traffic` 列表默认不返回该字段，通过 `?include=applied_upstream` 显式请求。

## Sync 边界

- 该改动仅影响本地路由判定，不涉及 rule sync。
- Group 规则/远端规则共享同一 resolver，故行为自动一致。
- traffic 同步（若开启）需要在 schema 里加入 `applied_upstream` 字段版本号迁移。

## 实现切分

### Phase 1：收敛判定函数

- 抽出 `should_use_upstream_proxy` 与 CONNECT/SOCKS 版本。
- 单元测试覆盖 host-only、proxy-only、both、both-ignored 四种组合。

### Phase 2：接入三条链路

- HTTP handler、CONNECT tunnel、SOCKS TCP 全部改调用 helper。
- h3 预判改用同一判定。
- traffic record 补 `applied_upstream`。

### Phase 3：E2E 与 Web

- 恢复 `test_routing_host_vs_proxy`、`test_priority_host_vs_proxy` 通过。
- 新增 SOCKS host-vs-proxy 用例。
- Web traffic detail 展示新字段。

### Phase 4：文档与迁移

- 更新 `docs/rules.md` 里 host/proxy 优先级说明。
- 更新 `human_tests/proxy-http-https.md`、`human_tests/proxy-socks.md`。

## 测试方案

### 单元测试

- `should_use_upstream_proxy_host_wins`
- `should_use_upstream_proxy_host_ignored_lets_proxy_win`
- `should_use_upstream_proxy_no_host_uses_proxy`
- `should_use_upstream_proxy_no_proxy_returns_false`
- `should_use_connect_upstream_proxy_matches_http_semantics`
- `should_use_socks_upstream_proxy_matches_http_semantics`
- `h3_disabled_only_when_upstream_proxy_wins`

### E2E 测试

- `cargo run -p bifrost-e2e -- --test routing_host_vs_proxy`
- `cargo run -p bifrost-e2e -- --test priority_host_vs_proxy`
- 新增 `cargo run -p bifrost-e2e -- --test socks_host_vs_proxy`
- 新增 `cargo run -p bifrost-e2e -- --test connect_host_vs_proxy`

### 真实场景测试

新增/更新 `human_tests/proxy-http-https.md`：

- TC-HPP-01：普通 HTTP，host+proxy 同时命中，抓包确认目标是 host 而非 proxy。
- TC-HPP-02：HTTPS CONNECT，同一场景验证隧道直连 host 目标而不是上游代理。
- TC-HPP-03：SOCKS，用 curl `--socks5` 或 `--socks5-hostname` 验证 host 优先。
- TC-HPP-04：`@ignore host` 后 proxy 恢复生效。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验与项目验证

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-proxy host_proxy_precedence -- --nocapture`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本地按 no-local-coverage 约定豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：host 优先、ignored 例外、CONNECT/SOCKS/h3 一致。
- 复核 diff：三条链路都替换为 helper，无遗留旧判定。
- 重点 review：`applied_upstream` 字段序列化兼容；日志噪音；SOCKS reply bind address 是否随 host 目标更新。
- 复测：单元 + 两条恢复 E2E + 新增 SOCKS/CONNECT E2E。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- `git status --short`、`git diff` 检查是否留下 debug print 或未删的旧分支。
- 重点 review：h3 预判是否真的按 host 优先允许启用；traffic 前端展示是否影响布局。
- 复测：失败路径重跑；使用真实浏览器和 curl 手动验证一次。

## 风险与决策点

- **反直觉风险**：部分用户可能希望“proxy 无脑覆盖所有请求”。文档需要明确 host 优先是全局稳定语义；如果要 proxy 覆盖，应从规则里删掉 host 或加 `@ignore host`。
- **CONNECT 隧道 host 目标是否走 TLS intercept**：本次不改变 TLS intercept 判定，只影响“走 host 还是走上游代理”。
- **SOCKS bind address**：host 优先场景返回目标真实 bind，不是 proxy 中转 bind；这与 SOCKS RFC 一致。
- **traffic schema 迁移**：`applied_upstream` 是新字段，旧数据缺省为 `direct`，无需破坏性 migration。
- **HTTP/3 探测**：若上游确实拒绝 h3，仍走既有 h3→h2 fallback，不因本次改动新增额外重试。
