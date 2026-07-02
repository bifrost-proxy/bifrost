# PAC Proxy Auto-Config 方案

## 背景

Bifrost 已经把 `pac` 注册为规则协议。本方案第一阶段把 `pac://` 从占位能力升级为可执行的 PAC 路由：支持内嵌/Values/本地文件/远程 URL 加载 PAC 脚本，执行 `FindProxyForURL(url, host)`，并把 `DIRECT`、`PROXY`、`HTTP`、`HTTPS` 决策映射到当前请求的直连或上游代理链路。

本方案把 `pac://` 设计为 Bifrost rules 的一等路由协议。用户通过规则显式声明哪些请求需要执行 PAC，PAC 脚本返回 `DIRECT`、`PROXY host:port`、`HTTPS host:port`、`SOCKS host:port` 或 `SOCKS5 host:port` 后，Bifrost 再把该决策映射到自身的上游路由/代理转发链路。

核心不变量：

- Bifrost 代理核心自身发起的出站 HTTP client 不读取系统代理或 `HTTP_PROXY` / `HTTPS_PROXY`。
- 用户需要代理转发时，必须通过 Bifrost rules 显式表达。
- PAC 只影响命中规则的被代理请求，不影响 Sync、upgrade、install-skill、AI provider、remote relay 等 Bifrost runtime outbound client。

## 目标

- 支持规则语法 `pattern pac://value [filters...]`。
- `value` 支持内联/内嵌/Values、本地文件路径、远程 URL。
- 执行标准 `FindProxyForURL(url, host)`，并提供常见 PAC helper。
- PAC 作用于规则替换后生成的 Final URL；Final URL 为空时作用于原始请求 URL。
- 支持 `enable://proxyHost` 高级语义：PAC 选择上游代理后，可指定上游代理实际连接的 host/IP/port。
- 保留 Bifrost rules 的过滤器、优先级、enabled 状态、Group/My Rules、临时端口绑定等现有控制面。
- 提供安全沙箱、缓存、超时、大小限制和可观测诊断，避免任意 PAC 脚本拖垮代理请求路径。

## 非目标

- 不把系统代理 PAC 配置导入为 Bifrost 默认出站配置。
- 不让 PAC 脚本访问文件系统、网络、环境变量、进程、系统 DNS 配置或 Bifrost 管理 API。
- 不在 PAC 脚本内支持异步 JavaScript、Promise、fetch、XMLHttpRequest 或 Node API。
- 不支持 PAC 脚本修改请求头、响应体或 TLS 拦截策略；这些继续由其它 rule protocol 负责。

## 当前实现状态

- `Protocol::Pac` 语法提示、规则文档和站点文档已从占位说明更新为可用路由协议。
- 旧的字面量 `PROXY host:port` -> `host` 路由映射已移除，PAC 决策现在作用于 `result.proxy`，`DIRECT` 会清除已有上游代理。
- `bifrost-script` 已新增 PAC 专用 rquickjs 执行器，提供常见 PAC helper、脚本大小限制和执行超时。
- `pac://{name}`、`pac:///abs/path.pac`、`pac://http(s)://...` 和短内联脚本均可作为 PAC 来源；远程 PAC 下载复用 Bifrost outbound client builder，仍不读取系统代理。
- 已补充单元测试和真实 E2E：Values PAC、远程 PAC、PAC + host/proxy 转发、双 Bifrost 上游代理链路。
- 尚未落地：`enable://proxyHost` 高级语义、Traffic/Overview PAC 诊断字段、连接失败后的多候选 fallback、WebUI 专用展示。

## 用户语法

### 内嵌 PAC 脚本

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

### Values

```txt
www.example.com/path1 pac://{test2.pac}
```

### 本地文件

```txt
www.example.com/path3 pac:///Users/eden/test.pac
```

### 远程 PAC 脚本

```txt
* pac://https://raw.githubusercontent.com/imweb/node-pac/master/test/scripts/normal.pac
```

### `proxyHost` 高级用法

```txt
www.example.com pac://https://example.com/normal.pac 1.1.1.1 enable://proxyHost
www.example.com pac:///Users/eden/test.pac 1.1.1.1:8080 enable://proxyHost
```

其中 `1.1.1.1` 等价于 `host://1.1.1.1`。只有 PAC 结果选择了上游代理时，`enable://proxyHost` 才让 Bifrost 把上游代理收到的目标 host/port 改为该值；如果 PAC 返回 `DIRECT`，该字段不生效。

## 规则解析模型

新增规则内部表示：

```rust
pub struct PacRuleConfig {
    pub source: ValueSource,
    pub proxy_host_override: Option<HostTarget>,
    pub proxy_host_enabled: bool,
}
```

解析规则：

- `pac://value` 的 `value` 直接复用 `ValueSource`。
- `pac://{name}` 解析为 Values/内嵌值引用。
- `pac:///abs/path.pac` 解析为本地文件路径。
- `pac://http://...` 和 `pac://https://...` 解析为远程 PAC URL。
- `pac://(...)` 可作为短内联脚本，但多行脚本推荐使用内嵌值或 Values。
- 同一行出现 host-like 裸值且同时出现 `enable://proxyHost` 时，解析为 `proxy_host_override`。
- 没有 `enable://proxyHost` 时，裸 host-like 值仍按既有 `host://` 语义处理，不绑定到 PAC。

为了避免与其它协议互相覆盖，`pac://` 应作为路由类协议参与 first-win/last-win 规则，建议与 `proxy://`、`host://`、`http://`、`https://` 同级；最终选中的 PAC 决策再转化为临时路由结果。

## Final URL 语义

PAC 评估必须使用规则替换后生成的 Final URL：

1. 对原始请求运行第一阶段路由解析，得到可能的 `host/http/https/ws/wss/tunnel/urlReplace` 等路由结果。
2. 根据第一阶段结果构造 Final URL。无法构造时回退到原始请求 URL。
3. 用 Final URL 重新选择可命中的 `pac://` 规则。
4. 执行 PAC 脚本时传入 `FindProxyForURL(final_url, final_host)`。
5. PAC 结果只产生代理/直连决策，不再次触发 URL rewrite，避免循环。

示例：

```txt
www.example.com/api www.example.com
www.example.com pac://https://example.com/normal.pac
```

请求 `https://www.example.com/api/path` 先被第一条规则转换为 `https://www.example.com/path`。第二条 PAC 规则对 Final URL 生效，因此 PAC 脚本看到的是 `https://www.example.com/path`。

如果用户写成单条：

```txt
www.example.com/api www.example.com pac://https://example.com/normal.pac
```

PAC 规则只匹配原始请求阶段的 `www.example.com/api`，不应在 rewrite 后再次用同一条规则命中 Final URL。文档应建议拆成两条规则。

## PAC 执行器

新增 `bifrost-core` 或独立 crate `bifrost-pac`：

```rust
pub struct PacEngine {
    cache: PacScriptCache,
    loader: PacScriptLoader,
    runtime_limits: PacRuntimeLimits,
}

pub struct PacDecision {
    pub mode: PacDecisionMode,
    pub chain: Vec<PacProxyHop>,
    pub raw: String,
}

pub enum PacDecisionMode {
    Direct,
    Proxy,
    FailClosed,
}
```

执行步骤：

1. 从 `ValueSource` 加载 PAC 脚本文本。
2. 以 `source_id + sha256(script)` 为 key 编译并缓存。
3. 用 rquickjs 创建 PAC 专用 runtime/context。
4. 注入 PAC helper。
5. 调用 `FindProxyForURL(url, host)`。
6. 解析返回值，按分号顺序得到候选链。
7. 选择第一个 Bifrost 支持的 hop；当前 hop 连接失败时，可以按候选链 fallback。

复用现有 `bifrost-script` 的 rquickjs 依赖是首选，但 PAC 执行器应保持 API 独立，不暴露 `net.fetch`、文件 API、请求/响应脚本上下文等 Bifrost script sandbox 能力。

## PAC helper

首期实现常见同步 helper：

- `isPlainHostName(host)`
- `dnsDomainIs(host, domain)`
- `localHostOrDomainIs(host, hostdom)`
- `isResolvable(host)`
- `dnsResolve(host)`
- `isInNet(host, pattern, mask)`
- `shExpMatch(str, shexp)`
- `weekdayRange(...)`
- `dateRange(...)`
- `timeRange(...)`
- `myIpAddress()`
- `alert(message)` 作为 debug no-op，可写入 trace debug log

DNS helper 使用 Bifrost 当前 resolver/DNS cache，必须设置短超时和请求级缓存，避免每个请求重复阻塞解析。

## PAC 返回值映射

支持返回项：

- `DIRECT`
- `PROXY host:port`
- `HTTP host:port`
- `HTTPS host:port`
- `SOCKS host:port`
- `SOCKS5 host:port`

映射策略：

- `DIRECT`：等价于不设置下游代理，继续直连当前 Final URL 的上游目标。
- `PROXY` / `HTTP`：写入 `resolved.proxy = http://host:port`。
- `HTTPS`：写入 `resolved.proxy = https://host:port`，需要 proxy connector 支持 HTTPS proxy；首期若不支持，应 fail-closed 并给出诊断。
- `SOCKS` / `SOCKS5`：如果现有上游代理 connector 支持 SOCKS，则映射；否则 fail-closed 并提示未支持。
- 多个候选用 `;` 分隔，按顺序尝试。第一阶段可只选择首个支持项，第二阶段再加入连接失败后的 fallback。

非法或空返回值：

- 默认 fail-closed，返回可诊断错误页/traffic error。
- 可在后续增加 `pacFailOpen://true`，但首期不建议默认 fail-open，避免企业代理规则被错误绕过。

## `enable://proxyHost`

新增 resolved 字段：

```rust
pub struct ProxyHostOverride {
    pub host: String,
    pub port: Option<u16>,
}
```

生效条件：

- 当前请求命中 PAC。
- PAC 返回上游代理 hop。
- 同一规则或合并后的最终路由启用了 `enable://proxyHost`。
- 存在 host override。

行为：

- HTTP absolute-form 请求：发送给上游代理的 request-target 使用 override host/port 生成 URL，但外层 Host 头和应用层 URL 保持 Final URL 语义，避免改变用户请求语义。
- HTTPS CONNECT：向上游代理发送 `CONNECT override_host:override_port`，同时客户端侧看到的目标域名、SNI 解包策略、Traffic 展示仍以 Final URL/原始请求为主；overview 中单独展示 `proxy_connect_target`。
- 如果 override 未指定端口，则使用 Final URL 推导端口：HTTPS 443，HTTP 80，或当前路由结果端口。

该设计满足“让上游代理根据指定 IP/端口继续请求”，同时保留 Bifrost 对原始目标和 Final URL 的可观测性。

## 安全与资源限制

- PAC 脚本最大默认 1 MiB；可通过配置上限调整，但不得超过全局脚本最大值。
- 远程 PAC 下载必须使用 Bifrost outbound trust builder，仍不读取系统代理。
- 远程 PAC 只允许 `http` / `https`，默认 5 秒连接超时、10 秒总超时。
- 编译缓存按 source key、etag/last-modified、sha256 和 TTL 管理。
- 执行超时默认 50 ms；DNS helper 总预算默认 200 ms。
- 单个请求最多执行一次 PAC；Final URL 二次匹配不能递归触发 PAC。
- JS runtime 不注入 `Date.now` 以外的随机或系统能力；时间类 helper 可使用当前本地时间。
- PAC 错误必须写入 traffic record：`pac_source`、`pac_result_raw`、`pac_decision`、`pac_error`、`pac_elapsed_ms`。

## 与现有模块的改造点

### bifrost-core

- 扩展 `Protocol::Pac` 语义描述，移除占位提示。
- 扩展 rule parser，保留 `pac://` value 的完整 `ValueSource`。
- 增加 PAC rule config 和 resolved PAC diagnostic 字段。
- 修改 resolver 为两阶段：原始请求解析 -> Final URL 构造 -> PAC 规则匹配与执行。
- 增加 `enable://proxyHost` 或等价 line prop 的解析与验证。

### bifrost-proxy

- 在 HTTP/CONNECT/SOCKS5 上游建立前消费 PAC 决策。
- 把 PAC `PROXY` 决策映射到现有 HTTP proxy chaining 逻辑。
- 在 `proxyHost` 生效时改写发给上游代理的目标，而不是改写客户端可见目标。
- Overview/Traffic 增加 PAC 决策展示。

### bifrost-script / bifrost-pac

- 建议新增 PAC 专用模块，不直接复用 request/response script sandbox。
- 如复用 rquickjs，应只复用底层 runtime 构建方式和测试工具，不复用 `net.fetch`、文件 API。

### CLI / Web UI

- CLI 语法检查能识别 PAC value 来源、`enable://proxyHost`、非法远程 scheme、缺失 `FindProxyForURL`。
- Web UI rule editor 给出 PAC 专用提示和诊断。
- Active rules summary 展示 PAC 来源类型：inline/value/file/remote。

### 文档

- 更新 `docs/rules/routing.md` 的 `pac` 章节。
- 更新 `docs/operation.md`，明确 PAC 推荐使用内嵌值/Values/文件/远程资源。
- 更新 `docs-en/operation.md` 对应英文说明。
- 更新 `SKILL.md` 的规则能力说明。

## 测试方案

### 单元测试

- `ValueSource` 解析 PAC 的内联、Values、文件、远程 URL。
- PAC helper：`dnsDomainIs`、`shExpMatch`、`isPlainHostName`、`isInNet`。
- PAC 返回值解析：`DIRECT`、`PROXY`、多候选、非法值、大小写和空白。
- Final URL 二阶段匹配：rewrite 后 PAC 命中；单条规则不递归二次命中。
- `enable://proxyHost`：端口推导、CONNECT target override、未启用时不生效。
- 安全限制：缺失 `FindProxyForURL`、执行超时、脚本过大、远程下载失败。

### E2E

- 内嵌 PAC：域名命中返回 `PROXY`，请求经下游 mock proxy。
- Values PAC：同一脚本被多条规则复用。
- 本地文件 PAC：修改文件后缓存失效并重新加载。
- 远程 PAC：通过私有 HTTP server 提供脚本，验证拉取、缓存和失败诊断。
- `DIRECT`：不走下游代理。
- Final URL：第一条规则 rewrite，第二条 PAC 对 rewrite 后 URL 生效。
- `enable://proxyHost`：上游 mock proxy 看到 CONNECT/absolute-form 目标是指定 IP/端口。
- PAC 错误页和 Traffic 诊断字段。

### human_tests

- 设计阶段新增 `human_tests/pac-proxy-auto-config-design.md`，验证方案是否覆盖规则语法、Final URL、`enable://proxyHost`、系统代理不变量、安全限制和测试计划。
- 实现阶段新增 `human_tests/pac-proxy-auto-config.md`，覆盖用户可感知语法、Overview 展示、Traffic 诊断、失败提示、系统代理不变量。

### coverage 90% 门禁

- 实现阶段必须为新增 PAC parser、decision parser、helper、Final URL resolver 和 proxyHost override 补充单元测试。
- 收尾运行 `make coverage`。若 E2E 环境不可用，至少运行 `make coverage-unit` 并记录原因。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户语法样例是否全部覆盖。
- review parser/resolver/PAC engine/proxy connector 的边界。
- 运行 PAC 单元测试和最小 E2E。
- 修复发现的问题。

第 2 轮：

- 复查 Final URL、proxyHost、安全沙箱、系统代理不变量和文档一致性。
- 复跑失败路径、完整 PAC E2E、human_tests。
- 运行 workspace all-features、coverage gate 和远端 CI。

若任一轮发现高频请求性能、代理链路安全、TLS/CONNECT 语义或系统代理边界问题，继续追加 Review/Fix/Test 轮次。

## 分阶段落地

### Phase 1：PAC engine 与静态决策

- 新增 PAC script loader、decision parser、rquickjs sandbox 和 helper。
- 支持内嵌/Values/文件/远程 URL。
- 支持 `DIRECT` / `PROXY`。
- 支持基础诊断。

### Phase 2：Final URL 与 proxyHost

- 引入两阶段 resolver。
- 支持 rewrite 后 Final URL 匹配 PAC。
- 支持 `enable://proxyHost` 和 Traffic 展示。

### Phase 3：候选链与更多代理类型

- 支持 PAC 返回多个候选并在连接失败时 fallback。
- 支持 HTTPS proxy、SOCKS/SOCKS5 上游代理能力或明确 fail-closed。
- 增加缓存 TTL、etag、手动刷新和 Web UI 诊断。

## 残余风险

- PAC 在请求热路径执行，必须严格控制缓存和超时，否则会放大尾延迟。
- DNS helper 的结果可能与上游代理所在网络不同；默认只作为脚本条件使用，不改变上游代理 DNS 解析，除非用户显式使用 `enable://proxyHost`。
- Final URL 二阶段匹配改变 routing pipeline，容易影响现有优先级和自动 TLS 解包，需要专门回归。
- 远程 PAC 下载失败的 fail-closed 策略更安全，但可能让迁移用户感觉比浏览器 PAC 更严格，需要文档解释。
