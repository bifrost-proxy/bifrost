# Host Rule Path Rewrite

## 背景

Bifrost 的 host rule 允许把 URL 从一个 origin/path 前缀重写到另一个 origin/path 前缀，常用于前端本地开发把线上静态资源换成本地 dev server。真实规则示例：

```text
https://internal.example.test/labor_cost/static/ http://localhost:9000/labor_cost/static/
https://internal.example.test/labor_cost/static/__webpack_hmr http://localhost:9000/__webpack_hmr
```

历史实现在提取“source path”时是从 `resolved_rules.rules` 倒序抓最后一条 host 类规则，再把 target path 当作 base path 拼接，导致两类线上事故：

1. 前缀重写场景下，请求 `/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg` 被错误映射为 `http://localhost:9000/labor_cost/static/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg`，浏览器 404。
2. 精确路径重写场景下，`__webpack_hmr` 被错误补上尾斜杠变成 `__webpack_hmr/`，本地 dev server 拒绝，HMR 断链。

同一路径被 HTTP forwarding、HTTPS TLS 拦截、WebSocket 握手三条链路各自实现，任何一条漏改都会让部分场景带病。本文档描述如何将 host rule path 重写收敛到单一 helper，并绑定“真正生效的源 pattern path”做裁剪。

## 用户目标验证清单

### 必须实现

- 目录前缀重写：`https://a/x/ -> http://b/x/`，请求 `a/x/f.svg` 转发到 `http://b/x/f.svg`，不重复前缀。
- 精确路径重写：`a/x/foo -> http://b/foo`，请求 `a/x/foo` 转发到 `http://b/foo`，不追加尾斜杠。
- Query string 保留原样。
- 源 pattern path 必须来自“真正生效的 host 规则”，不是倒序扫的最后一条。
- HTTP forwarding、HTTPS TLS intercept、WebSocket 握手三条链路都用统一 helper。
- 正则 pattern 只负责选中请求；当普通 `http://` / `https://` / `ws://` / `wss://` 转发目标带非根路径时，该目标 path/query 是权威资源地址，不能沿用原请求路径。

### 必须不破坏

- 只写目标 origin 不写路径的 host rule（`a -> http://b`）继续工作。
- host rule 与其他协议（`resHeaders://`、`resScript://` 等）叠加时行为一致。
- 大小写敏感度、编码后的路径（`%20` 等）、trailing slash 判定与原语义一致。
- WebSocket 握手请求头 `Host`、`Origin` 改写不受影响。
- 域名、IP 和普通 `*` 通配符规则继续使用既有 path 前缀裁剪/拼接语义。
- 正则规则只写目标 origin、不写资源路径时，仍只替换 origin 并保留原请求 path/query。
- `redirect://` 仍然是向客户端返回 3xx；普通转发规则的精确资源映射只在代理内部改写上游请求，不产生浏览器重定向。

### 必须真实验证

- 通过真实代理请求 `.svg` 静态资源，抓取上游 `parsed_path`，断言等于单份 `/labor_cost/static/<file>`。
- 通过真实代理请求 `__webpack_hmr`，断言上游收到的 path 精确等于 `/__webpack_hmr`。
- HTTPS + TLS 拦截 + 静态资源路径前缀保留通过本地 mitm echo server 验证。

## 产品语义

### 匹配器决定目标 path 语义

转发规则的左侧 pattern 同时承担“是否命中”和“能否安全提取字面 source path”两项职责：

- `Domain`、`IP`、普通 `Wildcard`：左侧包含可解释的字面 path，继续按 source → target 前缀映射，并保留未匹配后缀。
- `PathWildcard`：沿用现有兼容语义，目标非根 path 是精确目标。
- `Regex`：正则表达式不是字面路径，不能交给 `strip_prefix`。目标带非根 path 时直接使用目标 path/query；目标只有 origin 时保留原 path/query。

例如：

```text
/\/component-custom-mix-eu-fest-track-load-comp-index\.[^\/]*\.js/ http://127.0.0.1:9798/component-custom-mix-eu-fest-track-load-comp-index.js
```

请求中任意目录下的 hash 文件命中后，上游固定请求：

```text
http://127.0.0.1:9798/component-custom-mix-eu-fest-track-load-comp-index.js
```

这属于代理内部的 upstream URL 重写，不是 HTTP redirect。

### 源 pattern 与目标 path

用户写的规则 pattern（左侧）可以是：

- 只有 host：`api.example.com` → source path 为空。
- host + 目录路径：`api.example.com/x/` → source path `/x/`。
- host + 精确路径：`api.example.com/x/foo` → source path `/x/foo`。

目标 host rule value（右侧）可以是：

- 只有 origin：`http://b` → target path 为空。
- origin + 目录路径：`http://b/y/` → target path `/y/`。
- origin + 精确路径：`http://b/foo` → target path `/foo`。

重写规则：

1. 若 source path 是 request path 的前缀，去掉前缀，得到 tail。
2. 若 source path 尾部有 `/`，target path 也以 `/` 结尾时，用 `target_path + tail_without_leading_slash` 拼接；target path 不以 `/` 结尾时按目录语义补齐。
3. 若 source path 与 request path 精确相等（例如 `__webpack_hmr` 精确规则），直接用 target path，不做任何 tail 拼接、不补尾斜杠。
4. Query string 直接透传。

### 反查生效的 host 规则

`resolved_rules.rules` 里可能同时存在多条 host 规则（例如通配 + 精确）。历史逻辑倒序取最后一条 host 类规则，容易命中 index 上排在后面的宽泛规则。正确做法：

- 用当前请求最终生效的 `host_protocol` 和 `host_rule value`（即已经作为 `resolved_rules.host` 的那条），在 `resolved_rules.rules` 中从前向后查找第一条 `protocol == Host && value == 生效值` 的规则。
- 拿到该规则 pattern 的 source path 作为裁剪基准。
- 若该规则无 source path，则退化为“仅换 origin”的旧语义，request path 原样带过去。

## 技术细节

### 修改点

- `crates/bifrost-core/src/matcher/factory.rs`
  - 提供统一的“pattern 是否使用精确转发目标 path”判定，避免 Proxy、Replay、PAC 各自猜测。
- `crates/bifrost-proxy/src/utils/url.rs`
  - 新增 `rewrite_host_path(source_path: &str, target_path: &str, request_path: &str) -> String`。
  - 新增 `find_host_rule_source_path(resolved: &ResolvedRules) -> Option<String>`。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - HTTP forwarding 里 host 重写调用两个 helper。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - HTTPS TLS 拦截后的转发同样调用两个 helper。
- `crates/bifrost-proxy/src/proxy/websocket/handler.rs`
  - WS 握手 URL 组装调用同一 helper，保证 handshake path 一致。
- `crates/bifrost-proxy/src/traffic/record.rs`
  - Traffic detail 展示 host 重写命中的 source path 与 target path，便于排查。
- `crates/bifrost-admin/src/request_rules.rs`
  - Replay 使用同一 matcher 判定，保证重放与实时请求一致。
- `crates/bifrost-cli/src/parsing/rules.rs`
  - PAC 计算 Final URL 时使用同一 matcher 判定；精确目标的 query 以目标为准。

### helper 签名与语义

```rust
pub fn rewrite_host_path(source_path: &str, target_path: &str, request_path: &str) -> String {
    // 1. request_path 已经含 query? 由调用方拆好，这里只处理 path。
    // 2. exact match: source == request_path 直接返回 target_path。
    // 3. prefix match: source 以 `/` 结尾时按目录裁剪，剩余 tail 拼到 target 后。
    // 4. 非前缀命中: 返回 target_path + request_path（旧兜底）。
}

pub fn find_host_rule_source_path(resolved: &ResolvedRules) -> Option<String> {
    let host = resolved.host.as_ref()?;
    resolved.rules.iter().find(|r| r.protocol == Protocol::Host && r.value == host.value)
        .and_then(|r| pattern_path(&r.pattern))
}
```

### 边界

- source path 有 `?query` 时，调用方需先剥离 query 再传入 helper。
- target path 含 query 时保留（例如 `http://b/api?debug=1`），helper 只处理 path 部分，query 由外层合并。
- 大小写：source path 匹配按大小写敏感（与 HTTP path 语义一致）。
- 百分号编码：source 与 request 都保持原始编码后再比较，helper 不做 decode/encode。

## CLI/Web/Admin API

### CLI

- `bifrost rule check <file>` 若发现 host rule pattern 与 target 都写了不匹配前缀，警告 `note: source path "/x/" and target path "/y/" will only rewrite path prefix, not remap arbitrary paths`。

### Web

- Traffic detail 的 URL 面板显示 `Rewritten from … to …`，包含 source path 与 target path 摘要，方便排查上游 404。

### Admin API

- `GET /api/traffic/:id` 响应新增 `host_rewrite: { source_path, target_path, matched_pattern }`。

## Sync 边界

- 不涉及新配置项，不影响 rule sync。
- 若 traffic 同步开启，需要在 schema 加入 `host_rewrite` 字段的版本迁移；缺省字段兼容旧客户端。

## 实现切分

### Phase 1：helper 与单元测试

- 新增 `rewrite_host_path` / `find_host_rule_source_path`。
- 单元测试覆盖前缀、精确、无 source path、query 保留、大小写、百分号编码。

### Phase 2：接入三条链路

- HTTP forwarding、HTTPS tunnel、WebSocket 握手改调用 helper。
- Traffic record 增加字段。

### Phase 3：E2E

- 新增 `e2e-tests/tests/test_host_rule_path_rewrite.sh` 覆盖前缀与精确两种。
- 补充 HTTPS + TLS intercept 场景。

### Phase 4：文档

- 更新 `human_tests/proxy-http-https.md` 与 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `utils/url.rs`
  - `test_rewrite_path_same_source_target`
  - `test_rewrite_path_prefix_no_double_prefix`
  - `test_rewrite_path_with_query_string`
  - `test_rewrite_path_exact_match_preserves_target_without_trailing_slash`
  - `test_rewrite_path_source_without_path_keeps_request_path`
  - `test_rewrite_path_percent_encoded_preserved`
  - `test_find_host_rule_source_path_uses_selected_rule_not_later_host_rule`
  - `test_find_host_rule_source_path_returns_none_when_no_host_rule`
  - 正则目标带 path 时使用精确目标；目标只有 origin 时保留原 path。
  - 同 value 的其它规则不得改变真正生效 pattern 的 path 语义。
  - Replay 与 PAC Final URL 对正则精确资源映射保持一致。

### E2E 脚本

- `e2e-tests/tests/test_host_rule_path_rewrite.sh`
  - 构造 `https://.../labor_cost/static/ -> http://127.0.0.1:<echo>/labor_cost/static/`。
  - 通过真实代理请求 `.svg`。
  - 断言上游 `parsed_path` 等于单份 `/labor_cost/static/<file>`。
  - 断言 query string 保持不变。
  - 构造 `https://.../labor_cost/static/__webpack_hmr -> http://127.0.0.1:<echo>/__webpack_hmr`。
  - 断言上游 `parsed_path` 精确等于 `/__webpack_hmr`，不能变成 `/__webpack_hmr/`。
  - 增加 HTTPS TLS intercept 变体，验证 tunnel 链路一致。
  - 增加 WebSocket 握手变体，验证 handshake 上游收到正确 path。

### 真实场景测试

- 更新 `human_tests/proxy-http-https.md`
  - 新增 host rule 路径前缀回归用例。
  - 新增 host rule 精确路径不补尾斜杠回归用例。
  - 覆盖 HTTPS + TLS 拦截 + 静态资源路径前缀保留。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验要求

- 先执行新增 E2E 脚本。
- 再执行 Rust 相关单元测试：`cargo test -p bifrost-proxy rewrite_host_path`。
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：前缀不重复、精确不补尾斜杠、query 保留、三条链路一致、source path 来自真正生效规则。
- 复核 diff：`resolved_rules.rules` 反查逻辑是否绑定 host value；HTTPS tunnel 与 WebSocket 是否都改到 helper；traffic record 是否有 host_rewrite 字段。
- 重点 review：百分号编码路径与非 ASCII path 是否被 helper 破坏；query 拆分与合并是否漏 fragment。
- 复测：单元测试、E2E 脚本、真实浏览器验证前端 dev 场景。

### 第 2 轮

- 复核第 1 轮发现的问题的修复。
- 再次检查 `git status --short`、`git diff`；确认 human_tests 索引更新。
- 重点 review：错误消息是否稳定可测试；WebSocket 场景是否覆盖 wss；日志是否泄露路径中的敏感 token。
- 复测：失败路径重跑；用 curl + tls intercept 手动过一次。

## 风险与决策点

- **反直觉情况**：源与目标都有非空 path 时的“前缀裁剪”对不熟悉规则的用户可能仍难理解。文档需要用图示 + 例子说明。
- **精确路径判定**：仅当 source path == request path 时才走 exact 分支；如果 source 是 `/foo` 而 request 是 `/foo/bar`，仍按前缀语义。
- **WebSocket wss**：wss 握手走 HTTPS tunnel 后再切协议，因此 helper 只要在 handshake path 组装里调用即可，不需要再单独 patch。
- **traffic 字段迁移**：`host_rewrite` 为可选字段，旧客户端读取时忽略即可。
- **回退路径**：source 与 target 都无 path 时，退化为旧“换 origin”行为，最大兼容存量规则。
