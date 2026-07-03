# html/js/css 内容注入协议回归

## 背景

Bifrost 的 `htmlAppend://` / `htmlPrepend://` / `htmlBody://`、`jsAppend://` / `jsPrepend://` / `jsBody://`、`cssAppend://` / `cssPrepend://` / `cssBody://` 用于在响应体上按 Content-Type 分类做前置/追加/整段替换。这一族协议在实际使用中是前端 debug、灰度注入、A/B 埋点、vConsole 分发等场景的核心工具。真实规则示例：

```text
https://nextoncall.bytedance.net/ htmlAppend://{vconsole-inject}
*.qq.com/ htmlAppend://{vconsole-inject}
```

历史实现存在多个明确 bug：

1. `htmlAppend` 直接把注入内容拼到整个 HTML 字符串末尾，导致内容落在 `</html>` 之后，浏览器解析大型前端页面可能产生 DOM 位置错乱或渲染异常。
2. Badge 注入把 Merged Rules 作为 JSON 内联到 `<script>`；规则文本自身包含 `</script>` 时会提前闭合 Badge 脚本，规则文本被浏览器当成真实 HTML 解析。
3. `WildcardMatcher` 对带尾部 `/` 的通配域名（如 `*.qq.com/`）编译成“域名后已有 `/`，再追加 `(/.*)?`”，导致子路径请求需要两个连续斜杠才匹配；无尾斜杠 URL 漏匹配。
4. HTTPS 解包后的 tunnel 响应链路没有执行 `apply_content_injection`，`nextoncall.bytedance.net/assistant -> http://localhost:5173/assistant` 命中 `HtmlAppend + Http` 只记录规则命中，不改写响应体。
5. `mock/file/rawfile/template/status` 立即响应绕过响应体处理链，规则命中但生成资源未被注入。
6. 带 `Content-Encoding: gzip/deflate/br/zstd` 的响应被直接当作 UTF-8 HTML 拼接，生成“gzip 数据 + 明文脚本”的损坏响应。
7. HTTPS MITM tunnel 上游 HTTP/2 拉取 3.4MB PNG（`h5.news.qq.com/static/culture.shtml` 背景图 `mat1.gtimg.com/.../202309061523.png`）在约 180KB 后 `INTERNAL_ERROR`，浏览器报 `net::ERR_HTTP2_PROTOCOL_ERROR`，页面加载完成但白屏。

本文档描述这批相关缺陷的一体化修复，覆盖注入语义、匹配器、Badge 转义、tunnel 一致性、mock 一致性、Content-Encoding 完整性、HTTP/2 大 body fallback。

## 用户目标验证清单

### 必须实现

- `htmlAppend` 内容插入最后一个 `</html>` 之前；无 `</html>` 时回退到文档末尾追加。
- `htmlPrepend` 插入 `<html…>` 开始标签之后（内部前部），而不是 `<!doctype>` 或 `<html>` 之前。
- `htmlBody` 只替换 `<body>…</body>` 内部，保留 head/body 属性；无 body 时回退整段替换。
- JS/CSS 系列按各自 Content-Type 分派，`append/prepend/body` 语义与 HTML 系列一致。
- Content-Encoding gzip/deflate/br/zstd 响应先解压 → 注入 → 重新压缩；解压/重压缩失败降级 identity。
- HTTPS 解包后 tunnel 与普通 HTTP handler 走同一 `apply_content_injection`。
- Mock / immediate response（`status://`、`file://`、`tpl://`、`rawfile://`、`resBody://`）在返回前也执行内容注入链。
- Badge 内联 JSON 数据把 `</` 转义为 `<\/`，避免 `</script>` 逃逸。
- 通配域名带根路径（`*.qq.com/`）匹配根路径与任意子路径；非通配、无尾斜杠语义保持原样。
- HTTPS MITM tunnel + 普通 HTTP handler 都支持 HTTP/2 → HTTP/1.1 fallback，避免大体积上游 body 中途断流转发半截。

### 必须不破坏

- 三类协议的现有语义（Content-Type 分派、prepend/append/body）保持一致。
- 未命中内容注入协议的响应不做额外处理开销。
- SSE、HEAD、204/304 等无正文或流式场景不触发 HTTP/2 fallback。
- 现有 `test_badge_injection_e2e.sh` 与 `body_manipulation.rs` 用例通过。
- `bp://` 单协议不因本次修复而触发 TLS intercept。

### 必须真实验证

- vConsole 风格脚本注入到真实前端页面后 DOM 位置正确。
- gzip HTML 响应命中 `htmlAppend` 后客户端仍能按 gzip 解压。
- `culture.shtml` 页面在 HTTPS MITM tunnel 下背景 PNG 完整传输，`Image.naturalWidth / naturalHeight` 为 `1800x2544`，且无 `ERR_HTTP2_PROTOCOL_ERROR`。
- `*.qq.com/ htmlAppend://…` 对 `www.qq.com`、`www.qq.com/`、`news.qq.com/rain/…` 均命中。

## 产品语义

### htmlAppend

- 找到最后一个大小写不敏感 `</html>`；用 `String::insert_str` 在该位置之前插入。
- 无 `</html>` 时保留旧“末尾追加”兼容行为，避免破坏片段响应。
- `htmlAppend` 不负责 body 内部定位；body 级修改必须用 `htmlBody`。

### htmlPrepend

- 找到大小写不敏感 `<html…>` 开始标签，将注入内容插入该标签的 `>` 之后。
- 无 `<html>` 时回退到文档开头前置；无 doctype 的片段响应仍自动补 `<!DOCTYPE html>`。

### htmlBody

- 找到 `<body …>` 开始标签与最后一个 `</body>`，只替换其内部 HTML；保留 `<html>`、`<head>`、`<body>` 及 body 属性。
- 无 body 标签时回退整段替换。

### JS / CSS

- prepend 在开头、append 在末尾、body 替换整段。
- 只对 `application/javascript`、`text/javascript`、`text/css` 等对应 Content-Type 生效；不跨类型串用。

### Badge 内联 JSON

- Merged Rules 数据先解析并重新序列化为 JSON，再把 `</` 转义为 `<\/`。
- 异常规则数据回退为空数据，绝不把原始规则文本注入 HTML。

### 通配域名 + 根路径

- `WildcardMatcher` 对带路径的通配域名不再盲目追加 `(/.*)?`。
- 规则路径以 `/` 结尾时，生成的正则允许 URL 无路径、根路径或任意子路径。
- 规则路径不以 `/` 结尾时保持路径前缀语义。
- 通配符只匹配单级子域名（`*.qq.com` 匹配 `www.qq.com` 但不匹配 `a.b.qq.com`），与既有语义一致。

### HTTP/2 → HTTP/1.1 fallback

- 上游 HTTP/2 建连/发送阶段失败：可重试的 GET/HEAD 请求切回 HTTP/1.1。
- 已拿到 HTTP/2 响应头的响应体按完整性风险分流：
  - 已知长度且不超过 `max_body_buffer_size`：先完整探测再转发。
  - 未知长度文本响应：按上限探测。
  - 未知长度二进制、超过上限的大二进制：直接切 HTTP/1.1 fallback。
- SSE、HEAD、204/304、POST 等不安全或流式响应不触发 fallback。

### Content-Encoding 完整性

- 命中 HTML/JS/CSS 内容注入协议 + `Content-Encoding: gzip/deflate/br/zstd` 时：解压 → 注入 → 按原编码重压缩。
- 解压失败：跳过内容注入，保留原始压缩响应。
- 响应头规则删除或改写 `Content-Encoding` 时：注入后的 body 与最终响应头保持一致。
- 重压缩失败：降级为 identity 响应并移除 `Content-Encoding`。

## 技术细节

### 修改点

- `crates/bifrost-proxy/src/transform/body.rs`
  - `apply_html_injection` 处理 append/prepend/body 三种子协议。
  - `apply_js_injection` / `apply_css_injection` 按 Content-Type 分派。
  - `apply_content_injection` 统一入口，负责 Content-Encoding 解压 → 注入 → 重压缩。
- `crates/bifrost-proxy/src/transform/badge.rs`
  - Merged Rules JSON 序列化后 `</` → `<\/` 转义。
- `crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`
  - 响应处理链接入 `apply_content_injection`。
  - HTTP/2 → HTTP/1.1 fallback 判定 `h2_body_recovery_policy`。
- `crates/bifrost-proxy/src/proxy/http/handler.rs`
  - 普通 HTTP handler 相同 fallback 判定与 mock immediate response 前的注入调用。
- `crates/bifrost-rules/src/matcher/wildcard.rs`
  - `WildcardMatcher` 修复根路径正则生成。
- `crates/bifrost-e2e/src/tests/body_manipulation.rs`
  - 覆盖 append/prepend/body 三类矩阵 + gzip + delete encoding + mock。
- `e2e-tests/tests/test_badge_injection_e2e.sh`
  - 新增 vConsole 规则文本回归。
- `human_tests/proxy-rules-advanced.md`
  - 新增 / 更新 TC-PRA-25、TC-PRA-29A/B/C/D/E、TC-PRA-59、TC-PRA-60。

### fallback 判定接口

```rust
pub enum H2BodyRecoveryDecision {
    ProbeBounded,     // 已知长度小响应
    ProbeUnknownText, // 未知长度文本
    RetryHttp1,       // 未知/超大二进制
    Skip,             // SSE、HEAD、204/304、POST
}

pub fn h2_body_recovery_policy(req: &Request, res: &Response, cfg: &BodyCfg) -> H2BodyRecoveryDecision;
```

## CLI/Web/Admin API

### CLI

- `bifrost rule check <file>` 检测在同一响应上同时写 `htmlAppend` 与 `htmlBody` 时提示语义先后关系。
- `bifrost traffic get <id>` 输出 `content_injection: { protocol, ranges, bytes_before, bytes_after, encoding }` 便于排查。

### Web

- Traffic detail Body 面板增加 “Content injection” chip，展示协议名、字节前后大小、Content-Encoding。
- Rules editor 对 `*.<domain>/` 尾斜杠给出 tooltip 说明匹配语义。

### Admin API

- `GET /api/traffic/:id` 响应扩展 `content_injection` 字段。

## Sync 边界

- 规则语法未变化，rule sync 不受影响。
- traffic 同步 schema 新增 `content_injection` 字段，缺省 `null` 兼容旧客户端。

## 实现切分

### Phase 1：注入语义修复

- `apply_html_injection` append/prepend/body 三分支。
- Badge JSON 转义。
- 单元测试覆盖上述行为。

### Phase 2：匹配器与 Mock

- `WildcardMatcher` 根路径正则修复。
- `apply_content_injection` 在 mock / immediate response 也执行。

### Phase 3：HTTPS tunnel 与 Content-Encoding

- tunnel 响应链接入 `apply_content_injection`。
- gzip/br/deflate/zstd 解压 → 注入 → 重压缩，失败降级。

### Phase 4：HTTP/2 fallback

- `h2_body_recovery_policy` helper。
- tunnel + handler 两条链路接入。
- `culture.shtml` 场景真实回归。

## 测试方案

### 单元测试

- `test_html_injection_append`
- `test_html_injection_append_uses_last_html_close_case_insensitive`
- `test_html_injection_append_falls_back_to_document_end_without_html_close`
- `test_html_injection_prepend_inserts_after_html_open`
- `test_html_injection_prepend_uses_html_open_case_insensitive`
- `test_html_injection_body_replaces_body_inner_html`
- `test_html_injection_body_falls_back_to_entire_replace_without_body_element`
- `test_js_injection_append_prepend_and_body_replace`
- `test_css_injection_append_prepend_and_body_replace`
- `test_content_injection_ignores_protocols_when_response_type_differs`
- `test_html_injection_gzip_preserves_encoding`
- `test_badge_inline_rules_data_escapes_script_close_tag`
- `test_badge_inline_rules_data_falls_back_for_invalid_json`
- `test_h2_body_recovery_policy_probes_bounded_responses`
- `test_h2_body_recovery_policy_retries_large_or_unknown_binary`
- `test_h2_body_recovery_policy_skips_non_retryable_or_streaming`
- `test_prefix_wildcard_with_root_path_matches_subpaths`
- `test_parse_wildcard_root_path_pattern`

### E2E 测试

- `body_htmlAppend_script`
- `body_htmlAppend_gzip_response`
- `body_htmlAppend_gzip_response_delete_encoding`
- `body_content_injection_protocol_matrix`
- `body_content_injection_mock_resources`
- `body_https_htmlAppend_forwarded_http`
- `matcher_wildcard_root_path_html_append`
- `test_badge_injection_e2e.sh` 新增 vConsole 规则文本回归
- `test_https_interception_retries_h2_body_failure_with_http1`
- `test_http_handler_retries_h2_body_failure_with_http1`

### 真实场景测试

`human_tests/proxy-rules-advanced.md`：

- TC-PRA-25：vConsole 风格脚本片段注入。
- TC-PRA-29A：HTML/JS/CSS 三类协议矩阵。
- TC-PRA-29B：HTTPS 页面命中 `HtmlAppend` 并通过 `http://localhost` 上游转发时仍应注入。
- TC-PRA-29C：`file://` 与 `tpl://` 生成资源命中 HTML/JS/CSS 注入协议时仍应生效。
- TC-PRA-29D：gzip HTML 响应命中 `htmlAppend` 后仍是可解压 gzip，防止真实请求 `78778` 中出现压缩内容和明文脚本混拼。
- TC-PRA-29E：`*.qq.com/ htmlAppend://{vconsole-inject}` 对根路径与子路径页面均命中。
- TC-PRA-59：`culture.shtml` 在 HTTPS MITM tunnel 下加载完成但白屏的回归：背景 PNG 必须完整传输，`Image.naturalWidth / naturalHeight` 为 `1800x2544`，且无 `ERR_HTTP2_PROTOCOL_ERROR`。
- TC-PRA-60：本地可控 HTTP/2 upstream body 中途失败，分别验证 HTTPS MITM tunnel 与普通 HTTP handler 转 HTTPS upstream 都会通过 HTTP/1.1 fallback 返回完整 body。

按用例启动独立端口代理，创建 `htmlAppend` 规则，请求 HTML 文档并断言：

- `htmlAppend` 注入脚本出现在 `</html>` 之前；
- `htmlAppend` 注入脚本不会出现在 `</html>` 之后；
- `htmlPrepend` 内容出现在 `<html>` 开始标签之后；
- `htmlPrepend` 内容不会出现在 `<!doctype>` 或 `<html>` 之前；
- 原始 HTML 主体内容保持存在。

所有服务启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 校验要求

- `cargo test -p bifrost-proxy test_html_injection_append -- --nocapture`
- `cargo test -p bifrost-proxy test_html_injection_prepend -- --nocapture`
- `cargo test -p bifrost-proxy test_html_injection_gzip_preserves_encoding -- --nocapture`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_script`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_gzip_response`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_gzip_response_delete_encoding`
- `cargo run -p bifrost-e2e -- --test body_content_injection_protocol_matrix`
- `cargo run -p bifrost-e2e -- --test body_content_injection_mock_resources`
- `cargo run -p bifrost-e2e -- --test body_https_htmlAppend_forwarded_http`
- `cargo run -p bifrost-e2e -- --test matcher_wildcard_root_path_html_append`
- `bash e2e-tests/tests/test_badge_injection_e2e.sh`
- 按 `human_tests/proxy-rules-advanced.md` 执行 `TC-PRA-59` / `TC-PRA-60`
- `cargo test -p bifrost-proxy h2_body_recovery_policy -- --nocapture`
- `cargo test -p bifrost-tests --test https_proxy_test retries_h2_body_failure -- --nocapture`
- 按 `human_tests/proxy-rules-advanced.md` 执行 `TC-PRA-25`、`TC-PRA-29A`、`TC-PRA-29B`、`TC-PRA-29D`、`TC-PRA-29E`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机 no-local-coverage 约定下豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：append/prepend/body 三分支语义、Content-Type 分派、Content-Encoding 完整性、tunnel 与 handler 一致、mock 一致、Badge 转义、通配匹配、HTTP/2 fallback。
- 复核 diff：`transform/body.rs`、`transform/badge.rs`、tunnel / handler 是否都接入 `apply_content_injection`；`WildcardMatcher` 是否只在带路径场景改动正则。
- 重点 review：gzip 解压/重压缩失败降级；SSE 与 POST 不触发 fallback；`</` 转义是否覆盖 `</SCRIPT>` 大小写。
- 复测：单元 + focused E2E + human_tests culture.shtml。

### 第 2 轮

- 复查第 1 轮修复后的最新 diff、human_tests 索引、E2E 脚本。
- 重点 review：`*.qq.com/` 是否会影响其他既有通配规则；tunnel 里 `apply_content_injection` 顺序是否与 handler 完全一致；HTTP/2 fallback 是否会引入额外重试放大。
- 复测：全量 E2E 与 workspace test；curl + Playwright 手动验证 culture.shtml。

## 风险与决策点

- **`</` 转义边界**：只对内联 `<script>` 里的 JSON 转义；不影响普通 body 注入。
- **重压缩失败降级**：更改 `Content-Encoding` 会破坏严格检查上游，但保留部分 body 更符合用户预期，比返回错误更安全。
- **通配匹配语义**：`*.qq.com` 不覆盖多级子域名，符合既有语义；新增“根路径带尾斜杠”行为的兼容性通过单元测试锁定。
- **HTTP/2 fallback**：POST 与 SSE 明确跳过，避免副作用重放；`Idempotency-Key` 优先级不引入本次改动。
- **Body 预探测放大**：仅当已知长度小响应 / 未知长度文本时探测；未知长度大二进制直接切 HTTP/1.1，避免把大响应两次读取。
