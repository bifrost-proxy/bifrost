# PAC Proxy Auto-Config 方案

## 背景

Bifrost 已经把 `pac` 注册为规则协议。本方案把 `pac://` 从占位能力升级为可执行的 PAC 路由：支持内嵌 / Values / 本地文件 / 远程 URL 加载 PAC 脚本，执行标准 `FindProxyForURL(url, host)`，并把 `DIRECT` / `PROXY` / `HTTP` / `HTTPS` / `SOCKS` / `SOCKS5` 决策映射到 Bifrost 自身的直连或上游代理链路。

`pac://` 是 Bifrost rules 的一等路由协议。用户通过规则显式声明哪些请求需要执行 PAC，Bifrost 再把 PAC 决策转化为上游代理转发。

### 核心不变量

- **Bifrost 代理核心自身发起的出站 HTTP client 不读取系统代理或 `HTTP_PROXY` / `HTTPS_PROXY`**。
- 用户想要代理转发必须通过 Bifrost rules 显式表达。
- PAC 只影响命中规则的被代理请求，不影响 Sync、upgrade、install-skill、AI provider、remote relay 等 Bifrost runtime outbound client。

## 当前实现状态

- `Protocol::Pac` 语法提示、规则文档与站点文档已从占位说明升级为可用路由协议（`crates/bifrost-core/src/protocol.rs`、`crates/bifrost-core/src/syntax.rs`）。
- 旧的字面量 `PROXY host:port` → `host` 路由映射已移除，PAC 决策现在作用于 `result.proxy`；`DIRECT` 会清除已有上游代理。
- `bifrost-script` 新增 PAC 专用 rquickjs 执行器（`crates/bifrost-script/src/pac.rs`），提供常见 PAC helper、脚本大小限制、执行超时。
- `pac://{name}`、`pac:///abs/path.pac`、`pac://http(s)://...` 与短内联脚本均可作为 PAC 来源；远程 PAC 下载复用 Bifrost outbound client builder，仍不读取系统代理。
- 已补单元测试与真实 E2E：Values PAC、远程 PAC、PAC + host/proxy 转发、双 Bifrost 上游代理链路。
- **未落地**：`enable://proxyHost` 高级语义、Traffic/Overview 完整 PAC 诊断字段、多候选 fallback、Web UI 专用展示。

## 用户目标验证清单

### 必须实现

- 规则语法 `pattern pac://value [filters...]` 可解析可执行。
- `value` 支持内嵌 / Values / 本地文件路径 / 远程 URL。
- 执行标准 `FindProxyForURL(url, host)` 并提供常见 PAC helper。
- PAC 作用于规则替换后生成的 Final URL；Final URL 无法构造时回退到原始请求 URL。
- 单个请求最多执行一次 PAC；Final URL 二次匹配不递归触发 PAC。
- 沙箱、缓存、超时、大小限制、可观测诊断一应俱全。

### 必须不破坏

- Bifrost outbound HTTP client 仍不读取系统代理。
- 现有 rule 协议 `host://` / `proxy://` / `http://` / `https://` / `urlReplace://` / `tunnel://` 与 first-win/last-win 优先级不变。
- Bifrost script sandbox（`net.fetch`、request/response scripts）与 PAC 执行器互不影响。
- 现有临时端口绑定、Group 规则、matcher priority 不受影响。

### 必须真实验证

- 单元测试覆盖 PAC parser、helper、decision 解析、Final URL 二阶段、脚本超时、脚本过大、远程失败。
- E2E 用真实 mock proxy 验证 `DIRECT` / `PROXY` / 多候选 fallback。
- human_tests 覆盖内嵌 / Values / 文件 / 远程 4 种来源与失败诊断。

## 产品语义

### 用户语法示例

内嵌 PAC 脚本：

````txt
``` test.pac
function FindProxyForURL(url, host) {
  if (dnsDomainIs(host, ".internal.example.com")) {
    return "PROXY proxy.internal:3128";
  }
  return "DIRECT";
}
```

www.example.com/path pac://{test.pac}
````

Values：

```txt
www.example.com/path1 pac://{test2.pac}
```

本地文件：

```txt
www.example.com/path3 pac:///Users/eden/test.pac
```

远程 PAC 脚本：

```txt
* pac://https://raw.githubusercontent.com/imweb/node-pac/master/test/scripts/normal.pac
```

`proxyHost` 高级用法（`enable://proxyHost` 尚未落地）：

```txt
www.example.com pac://https://example.com/normal.pac 1.1.1.1 enable://proxyHost
www.example.com pac:///Users/eden/test.pac 1.1.1.1:8080 enable://proxyHost
```

其中 `1.1.1.1` 等价于 `host://1.1.1.1`。仅在 PAC 返回上游代理时，`enable://proxyHost` 才让 Bifrost 把上游代理连接的目标 host/port 改为该值；PAC 返回 `DIRECT` 时该字段不生效。

### PAC 返回值映射

| PAC 返回 | Bifrost 行为 |
| --- | --- |
| `DIRECT` | 清除已有上游代理，继续直连 Final URL 上游目标。 |
| `PROXY host:port` / `HTTP host:port` | `resolved.proxy = http://host:port`。 |
| `HTTPS host:port` | `resolved.proxy = https://host:port`；HTTPS proxy connector 支持时生效，否则 fail-closed。 |
| `SOCKS host:port` / `SOCKS5 host:port` | 由上游代理 connector 支持时映射；否则 fail-closed。 |
| 多个候选 `A; B; C` | 顺序尝试；第一阶段只选首个支持项，Phase 3 补 fallback。 |
| 非法 / 空值 | fail-closed，返回可诊断错误页 / traffic error。 |

## 技术细节

### 规则内部表示

```rust
pub struct PacRuleConfig {
    pub source: ValueSource,
    pub proxy_host_override: Option<HostTarget>,
    pub proxy_host_enabled: bool,
}
```

解析规则（`crates/bifrost-cli/src/parsing/rules.rs`）：

- `pac://value` 直接复用 `ValueSource`。
- `pac://{name}` → Values / 内嵌值引用。
- `pac:///abs/path.pac` → 本地文件路径。
- `pac://http://...` / `pac://https://...` → 远程 URL。
- `pac://(...)` → 短内联脚本（多行推荐使用内嵌值 / Values）。
- 同行 host-like 裸值 + `enable://proxyHost` → `proxy_host_override`。
- 无 `enable://proxyHost` 时，裸 host-like 值仍按既有 `host://` 语义处理。

`pac://` 与 `proxy://` / `host://` / `http://` / `https://` 同级参与 first-win/last-win；PAC 决策转化为临时路由结果。

### Final URL 语义

PAC 评估必须使用规则替换后生成的 Final URL：

1. 原始请求跑第一阶段路由解析，得到可能的 `host/http/https/ws/wss/tunnel/urlReplace` 结果。
2. 根据第一阶段结果构造 Final URL；无法构造回退到原始 URL。
3. Final URL 重新匹配可命中的 `pac://` 规则。
4. 执行 `FindProxyForURL(final_url, final_host)`。
5. PAC 结果只产生代理/直连决策，不再次触发 URL rewrite，避免循环。

示例：

```txt
www.example.com/api www.example.com
www.example.com pac://https://example.com/normal.pac
```

请求 `https://www.example.com/api/path` 先被第一条转成 `https://www.example.com/path`；第二条 PAC 对 Final URL 生效，脚本看到的是 `https://www.example.com/path`。

单条同时含 rewrite + PAC：

```txt
www.example.com/api www.example.com pac://https://example.com/normal.pac
```

PAC 只匹配原始请求阶段的 `www.example.com/api`，不再对 rewrite 后 URL 二次命中同一条规则；文档建议拆成两条。

### PAC 执行器（`crates/bifrost-script/src/pac.rs`）

```rust
pub enum PacDecision { /* ... */ }

pub enum PacProxyScheme {
    Direct, Http, Https, Socks, Socks5,
}

pub struct PacEngineConfig { /* size limit, timeout, dns budget */ }
pub struct PacEngine { /* cache, loader, runtime_limits */ }

impl PacEngine {
    pub fn new(config: PacEngineConfig) -> Self;
    pub fn evaluate(&self, script: &str, url: &str, host: &str) -> Result<PacDecision>;
}

pub fn parse_pac_decision(raw: &str) -> Result<PacDecision>;
```

执行步骤：

1. 从 `ValueSource` 加载 PAC 脚本文本。
2. 以 `source_id + sha256(script)` 为 key 编译并缓存。
3. rquickjs 创建 PAC 专用 runtime/context。
4. 注入 PAC helper。
5. 调用 `FindProxyForURL(url, host)`。
6. 按分号顺序解析候选链。
7. 选择第一个 Bifrost 支持的 hop；未来 Phase 3 允许失败后 fallback。

复用 `bifrost-script` 的 rquickjs 底层依赖，但 PAC 执行器 API 独立，不暴露 `net.fetch`、文件 API、请求/响应脚本上下文。

### PAC helper（首期）

`crates/bifrost-script/src/pac.rs` 中已注册：

- `isPlainHostName(host)`
- `dnsDomainIs(host, domain)`
- `localHostOrDomainIs(host, hostdom)`
- `isResolvable(host)`
- `dnsResolve(host)`
- `isInNet(host, pattern, mask)`
- `shExpMatch(str, shexp)`
- `weekdayRange(...)` / `dateRange(...)` / `timeRange(...)`
- `myIpAddress()`
- `alert(message)` → debug no-op / trace log

DNS helper 使用 Bifrost 当前 resolver / DNS cache，必须短超时 + 请求级缓存，避免每个请求阻塞解析。

### `enable://proxyHost`（Phase 2 待落地）

```rust
pub struct ProxyHostOverride {
    pub host: String,
    pub port: Option<u16>,
}
```

生效条件：

- 当前请求命中 PAC。
- PAC 返回上游代理 hop（非 DIRECT）。
- 同一规则或合并后最终路由启用 `enable://proxyHost`。
- 存在 host override。

行为：

- HTTP absolute-form 请求：发给上游代理的 request-target 使用 override host/port 生成 URL；外层 Host 头 + 应用层 URL 保持 Final URL 语义。
- HTTPS CONNECT：向上游代理发送 `CONNECT override_host:override_port`；客户端可见目标域名 / SNI / Traffic 展示仍以 Final URL 为主；overview 单独展示 `proxy_connect_target`。
- 未指定端口：从 Final URL 推导端口（HTTPS 443、HTTP 80、或当前路由端口）。

### CLI + Web + Admin API

- CLI：`bifrost rule check` 已能识别 PAC value 来源、非法远程 scheme、缺失 `FindProxyForURL`；`enable://proxyHost` 待补。
- Web：Rule editor 已允许 PAC 语法；专用诊断展示待 Phase 3。
- Admin API：`POST /api/rules` / `PUT /api/rules/:name` 直接接受 PAC 规则文本；不新增 endpoint。
- Traffic：`pac_source` / `pac_result_raw` / `pac_decision` / `pac_error` / `pac_elapsed_ms` 已部分落地，完整字段待 Phase 3。

### Sync 边界

- PAC 规则通过普通规则同步；PAC 脚本字面量随规则文本传输。
- 远程 PAC 引用的 URL 由每台设备各自下载；不通过 Sync 缓存脚本内容。
- Group 规则可以包含 PAC 规则；语义相同。

## Phase 1-4

### Phase 1：PAC engine 与静态决策（已落地）

- PAC script loader、decision parser、rquickjs sandbox、helper。
- 支持内嵌 / Values / 文件 / 远程 URL。
- 支持 `DIRECT` / `PROXY`。
- 支持基础诊断字段。

### Phase 2：Final URL 与 proxyHost

- 引入两阶段 resolver：原始 → Final URL → PAC 匹配。
- `enable://proxyHost` 落地：ProxyHostOverride 数据结构 + HTTP absolute-form + HTTPS CONNECT 目标改写。
- Traffic 展示 `proxy_connect_target`。

### Phase 3：候选链与更多代理类型

- PAC 多候选连接失败后 fallback。
- HTTPS proxy / SOCKS5 上游代理能力或明确 fail-closed。
- 缓存 TTL、etag、手动刷新、Web UI 诊断展示。

### Phase 4：文档与运维

- 更新 `docs/rules/routing.md`、`docs/operation.md`、`docs-en/operation.md` PAC 段落。
- 更新 `SKILL.md` 规则能力说明。
- 更新 human_tests。

## 测试方案

### 单元测试

位置：`crates/bifrost-script/src/pac.rs` `#[cfg(test)]`（脚本 line 397 起有实际 `FindProxyForURL` 测试样例）。

- ValueSource 解析 PAC 的内联 / Values / 文件 / 远程 URL。
- PAC helper：`dnsDomainIs`（line 170）、`shExpMatch`、`isPlainHostName`（line 162）、`isInNet`（line 217）。
- PAC 返回值解析：`DIRECT`、`PROXY`、多候选、非法值、大小写与空白。
- Final URL 二阶段匹配：rewrite 后 PAC 命中；单条规则不递归二次命中。
- `enable://proxyHost`：端口推导、CONNECT target override、未启用时不生效。
- 安全限制：缺失 `FindProxyForURL`（line 91-92 已断言错误消息）、执行超时、脚本过大、远程下载失败。

### E2E 测试

- 内嵌 PAC：域名命中返回 `PROXY`，请求经下游 mock proxy。
- Values PAC：同一脚本被多条规则复用。
- 本地文件 PAC：修改文件后缓存失效并重新加载。
- 远程 PAC：私有 HTTP server 提供脚本，验证拉取、缓存与失败诊断。
- `DIRECT`：不走下游代理。
- Final URL：第一条规则 rewrite，第二条 PAC 对 rewrite 后 URL 生效。
- `enable://proxyHost`：上游 mock proxy 收到 CONNECT / absolute-form 目标是指定 IP/端口。
- PAC 错误页 + Traffic 诊断字段。
- 现有 `crates/bifrost-e2e/src/tests/protocols.rs` 与 `matchers.rs` 已覆盖 PAC 基础路径；Phase 2/3 需要新增 `pac_proxy_host_override` 与 `pac_multi_candidate_fallback` 用例。

### human_tests

- `human_tests/pac-proxy-auto-config-design.md`：验证方案是否覆盖规则语法、Final URL、`enable://proxyHost`、系统代理不变量、安全限制与测试计划。
- `human_tests/pac-proxy-auto-config.md`：
  - TC-PAC-01：内嵌 PAC 返回 `PROXY` 经 mock proxy。
  - TC-PAC-02：Values PAC 多规则共用。
  - TC-PAC-03：本地文件 PAC 更新后缓存失效。
  - TC-PAC-04：远程 PAC 拉取 + 缓存 + 下载失败 fail-closed 诊断。
  - TC-PAC-05：Final URL 二阶段命中。
  - TC-PAC-06：`enable://proxyHost` CONNECT 目标改写（Phase 2 后启用）。
  - TC-PAC-07：`bifrost` outbound 客户端不读取系统代理（sanity check）。

### Coverage 门禁

- 实现阶段必须为新增 PAC parser、decision parser、helper、Final URL resolver、proxyHost override 补齐单元测试。
- 收尾运行 `make coverage`；E2E 环境不可用退化 `make coverage-unit` 并记录原因。

### 校验要求

- `cargo test -p bifrost-script pac`
- `cargo test -p bifrost-core pac`
- `cargo test -p bifrost-e2e protocols::pac`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo fmt --all -- --check`
- `rust-project-validate`

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户语法样例全部覆盖。
- Review parser / resolver / PAC engine / proxy connector 边界。
- 跑 PAC 单元 + 最小 E2E。
- 修复发现的问题。

### 第 2 轮

- 复查 Final URL、proxyHost、安全沙箱、系统代理不变量、文档一致性。
- 复跑失败路径、完整 PAC E2E、human_tests。
- 跑 workspace all-features、coverage gate、远端 CI。

若任一轮发现高频请求性能、代理链路安全、TLS/CONNECT 语义或系统代理边界问题，追加轮次。

## 风险与决策

- **决策**：PAC 只影响命中规则的请求；Bifrost outbound client 永不读系统代理。这是产品硬约束。
- **决策**：PAC 执行器独立于 script sandbox，不暴露 `net.fetch` / 文件 API。
- **决策**：默认 fail-closed；不引入 `pacFailOpen://true`。
- **风险**：PAC 在请求热路径执行，必须严格控制缓存与超时；执行超时默认 50 ms、DNS helper 总预算默认 200 ms；PAC 脚本默认最大 1 MiB。
- **风险**：DNS helper 结果可能与上游代理所在网络不同；默认只作为脚本条件，不改变上游代理 DNS 解析，除非 `enable://proxyHost`。
- **风险**：Final URL 二阶段改变 routing pipeline，容易影响优先级与自动 TLS 解包，必须专门回归。
- **风险**：远程 PAC 下载失败 fail-closed 更安全，但迁移用户可能觉得比浏览器 PAC 更严格；文档必须解释。
