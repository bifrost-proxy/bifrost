# html/js/css 内容注入协议回归

## 功能模块说明

`htmlAppend://` 用于向 `text/html` 响应注入 HTML 片段或脚本。对完整 HTML 文档，追加内容必须放在 HTML 元素内部的后部，优先插入最后一个 `</html>` 结束标签之前；如果响应没有 `</html>`，保持兼容行为，回退到文档末尾追加。

同一响应注入链还包含：

- `htmlPrepend://` / `htmlBody://`：只对 `text/html` 或 `application/xhtml` 响应生效，分别执行 HTML 元素内部前部插入和 `<body>...</body>` 内部替换。
- `jsAppend://` / `jsPrepend://` / `jsBody://`：只对 JavaScript Content-Type 响应生效，分别执行末尾追加、开头前置和整段替换。
- `cssAppend://` / `cssPrepend://` / `cssBody://`：只对 `text/css` 响应生效，分别执行末尾追加、开头前置和整段替换。

本次问题来自真实规则：

```text
https://nextoncall.bytedance.net/ htmlAppend://{vconsole-inject}
```

预期是把 vConsole 脚本追加到 HTML 文档响应的 `</html>` 之前。旧实现直接把内容拼到整个 HTML 字符串末尾，导致注入内容落在 `</html>` 之后，浏览器解析大型前端页面时可能产生 DOM 位置错乱或渲染异常。`htmlAppend` 不负责 body 内部定位，body 级修改应由专用协议处理。

排查过程中还发现 Badge 注入会把 Merged Rules 作为 JSON 放入内联 `<script>`。当规则文本自身包含 `</script><script>...` 时，浏览器 HTML 解析器会提前闭合 Badge 脚本，即使它位于 JS 字符串内部。因此 Badge 内联数据需要把 `</` 转义为 `<\/`。

本次白屏回归来自真实页面 `https://h5.news.qq.com/static/culture.shtml`。该页面主体文字为空，首屏内容完全依赖 `https://mat1.gtimg.com/www/pics/hv1/62/63/2304/202309061523.png` 背景图。HTTPS MITM tunnel 使用上游 HTTP/2 拉取该 3.4MB PNG 时，客户端会在约 180KB 后收到 `HTTP/2 stream ... INTERNAL_ERROR` / `transfer closed with ... bytes remaining`，浏览器报告 `net::ERR_HTTP2_PROTOCOL_ERROR`，导致页面加载完成但视觉为空白。

## 实现逻辑

- `apply_html_injection` 处理 `htmlAppend` 时先查找最后一个大小写不敏感的 `</html>`。
- 找到时用 `String::insert_str` 将注入内容插入该位置之前。
- 找不到 `</html>` 时回退为旧行为，在文档末尾追加，避免破坏片段 HTML 或非标准响应。
- `apply_html_injection` 处理 `htmlPrepend` 时先查找大小写不敏感的 `<html...>` 开始标签。
- 找到时将注入内容插入开始标签的 `>` 之后，确保内容位于 `<html>` 和 `</html>` 之间的前部，而不是 `<!doctype>` 或 `<html>` 之前。
- 找不到 `<html>` 时回退为旧行为，在文档开头前置；无 doctype 的片段响应仍自动补 `<!DOCTYPE html>`。
- `htmlBody` 先查找 `<body ...>` 开始标签和最后一个 `</body>`，找到时只替换 body 标签之间的内部 HTML，保留 `<html>`、`<head>`、`<body>` 及 body 属性；找不到 body 标签时才回退为整段替换。
- JS/CSS 系列保持现有字符串语义：prepend 在开头、append 在末尾、body 替换整段。
- 当响应带有 `Content-Encoding: gzip/deflate/br/zstd` 且命中 HTML/JS/CSS 内容注入协议时，响应处理链必须先按编码安全解压，再执行 `apply_content_injection`，最后按原编码重新压缩。不能把压缩字节当作 UTF-8 HTML 直接拼接，否则会生成“gzip 数据 + 明文脚本”的损坏响应。
- 如果解压失败，跳过内容注入并保持原始压缩响应；如果响应头规则删除或改写了 `Content-Encoding`，内容注入后的响应体必须和最终响应头保持一致；如果重压缩失败，降级为 identity 响应并移除 `Content-Encoding`。
- HTTPS 解包后的 tunnel 响应链路必须和普通 HTTP 响应链路一样执行 `apply_content_injection`；否则 `https://nextoncall.bytedance.net/assistant -> http://localhost:5173/assistant` 这类命中 `HtmlAppend + Http` 的请求只会记录规则命中，不会改写 HTML 响应体。
- `mock/file/rawfile/template/status` 这类由规则直接生成的立即响应也必须在返回前执行同一套响应体处理链；否则它们会因为提前返回绕过 `html/js/css` 内容注入协议，表现为规则命中但生成资源未被追加、前置或替换。
- Badge 内联规则数据先解析并重新序列化为 JSON，再把 `</` 转义为 `<\/`，避免规则文本逃逸为真实页面脚本。
- HTTPS MITM tunnel 和普通 HTTP handler 仍默认允许上游 HTTP/2；当可重试的 GET/HEAD 请求在上游 HTTP/2 建连/发送阶段失败，或在响应体发送给客户端前探测读取失败时，切换到 HTTP/1.1 fallback，避免把半截 body 转发给客户端。
- 对已拿到 HTTP/2 响应头的响应体，按完整性风险分流：已知长度且不超过 `max_body_buffer_size` 的响应会先完整探测；未知长度文本响应会按上限探测；未知长度二进制和超过上限的大二进制响应会在流式转发前直接重试 HTTP/1.1，避免首个大资源请求在客户端侧中途断流。SSE、HEAD、204/304 等无正文或流式场景不做重试。

## 依赖项

- `crates/bifrost-proxy/src/transform/body.rs`
- `crates/bifrost-proxy/src/transform/badge.rs`
- `crates/bifrost-e2e/src/tests/body_manipulation.rs`
- `e2e-tests/tests/test_badge_injection_e2e.sh`
- `human_tests/proxy-rules-advanced.md`

## 测试方案

### 单元测试

- `test_html_injection_append`：验证标准 `<html><body>...</body></html>` 中注入内容位于 `</html>` 之前。
- `test_html_injection_append_uses_last_html_close_case_insensitive`：验证大小写不敏感，并选择最后一个 `</html>`，避免误插入模板片段内的结束标签。
- `test_html_injection_append_falls_back_to_document_end_without_html_close`：验证无 `</html>` 时保持末尾追加兼容行为。
- `test_html_injection_prepend_inserts_after_html_open`：验证 `htmlPrepend` 插入 `<html...>` 开始标签之后。
- `test_html_injection_prepend_uses_html_open_case_insensitive`：验证 `htmlPrepend` 对 `<HTML>` 大小写不敏感。
- `test_html_injection_body_replaces_body_inner_html`：验证 `htmlBody` 只替换 `<body>...</body>` 内部，保留 head 和 body 属性。
- `test_html_injection_body_falls_back_to_entire_replace_without_body_element`：验证无 body 标签时保持整段替换兼容行为。
- `test_js_injection_append_prepend_and_body_replace`：验证 JavaScript append/prepend/body 三种协议及 Content-Type 分发。
- `test_css_injection_append_prepend_and_body_replace`：验证 CSS append/prepend/body 三种协议及 Content-Type 分发。
- `test_content_injection_ignores_protocols_when_response_type_differs`：验证不同系列协议不会跨 Content-Type 串用。
- `test_html_injection_gzip_preserves_encoding`：验证 gzip HTML 响应先解压注入再重新压缩，解压后的脚本位于 `</html>` 前，且仍保持有效 gzip。
- `test_badge_inline_rules_data_escapes_script_close_tag`：验证 Badge 中的规则文本不会因 `</script>` 提前闭合脚本。
- `test_badge_inline_rules_data_falls_back_for_invalid_json`：验证异常规则数据回退为空数据。
- `test_h2_body_recovery_policy_probes_bounded_responses`：验证 HTTP/2 已知小响应和未知长度文本响应会在转发前探测完整性。
- `test_h2_body_recovery_policy_retries_large_or_unknown_binary`：验证未知长度二进制和超过 buffer 上限的大二进制响应会在流式转发前切到 HTTP/1.1 fallback。
- `test_h2_body_recovery_policy_skips_non_retryable_or_streaming`：验证 POST、SSE 等不可安全重试或流式响应不触发 fallback。

### E2E 测试

- `body_htmlAppend_script`：通过 mock HTML 响应与真实代理请求验证 `htmlAppend` 输出完整 HTML 顺序，断言脚本在 `</html>` 前。
- `body_htmlAppend_gzip_response`：通过真实代理请求验证 gzip HTML 响应命中 `htmlAppend` 后，客户端仍能按 `Content-Encoding: gzip` 正常解压，且脚本注入到 `</html>` 前。
- `body_htmlAppend_gzip_response_delete_encoding`：验证 gzip HTML 响应同时命中 `htmlAppend` 与删除 `Content-Encoding` 规则时，返回体为可直接读取的 identity HTML，避免 gzip 字节和最终响应头不一致。
- `body_content_injection_protocol_matrix`：通过本地上游返回真实 `text/html`、`application/javascript`、`text/css` 响应，验证三类协议的 append/prepend/body 矩阵，其中 `htmlPrepend` 断言插入 `<html>` 之后，`htmlBody` 断言只替换 body 内部。
- `body_content_injection_mock_resources`：验证 `file://`/`tpl://` 等规则生成的 mock 响应在普通 HTTP handler 的提前返回路径中也会执行 HTML/JS/CSS 内容注入。
- `body_https_htmlAppend_forwarded_http`：验证 HTTPS 解包请求通过 `http://localhost` 上游转发后仍执行 `htmlAppend`，覆盖真实请求 `REQ-69f08a65-002153` 暴露的 tunnel 漏处理问题。
- `test_badge_injection_e2e.sh`：新增 vConsole 规则文本回归，验证 Merged Rules 能展示规则文本且不会把 `</script>` 逃逸为真实脚本。
- `culture.shtml` 真实页面回归：启动独立数据目录和非 9900 端口，使用 TLS 域名白名单拦截 `h5.news.qq.com`、`mat1.gtimg.com`、`vm.gtimg.cn`，通过 curl 和 Playwright 验证 PNG 背景图完整传输且浏览器渲染非空白。
- `test_https_interception_retries_h2_body_failure_with_http1`：构造本地 TLS 上游，HTTP/2 响应体发送部分数据后 reset，验证 HTTPS MITM tunnel 会重试 HTTP/1.1 并返回完整 body。
- `test_http_handler_retries_h2_body_failure_with_http1`：使用同一个本地 TLS 上游覆盖普通 HTTP handler 转 HTTPS upstream 的路径，验证非 CONNECT 请求也不会把半截 HTTP/2 body 转发给客户端。

### 真实场景测试

- 更新 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-25`，覆盖 vConsole 风格脚本片段注入。
- 更新 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-29A`，覆盖 HTML/JS/CSS 三类协议矩阵。
- 新增 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-29B`，覆盖 HTTPS 页面命中 `HtmlAppend` 并通过 `http://localhost` 上游转发时仍应注入。
- 新增 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-29C`，覆盖 `file://` 与 `tpl://` 生成资源命中 HTML/JS/CSS 注入协议时仍应生效。
- 新增 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-29D`，覆盖 gzip HTML 响应命中 `htmlAppend` 后仍是可解压 gzip，防止真实请求 `78778` 中出现压缩内容和明文脚本混拼。
- 新增 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-59`，覆盖 `culture.shtml` 在 HTTPS MITM tunnel 下加载完成但白屏的回归：背景 PNG 必须完整传输，浏览器中 `Image.naturalWidth` / `naturalHeight` 必须为 `1800x2544`，且无 `ERR_HTTP2_PROTOCOL_ERROR`。
- 新增 `human_tests/proxy-rules-advanced.md` 的 `TC-PRA-60`，覆盖本地可控 HTTP/2 upstream body 中途失败场景，分别验证 HTTPS MITM tunnel 和普通 HTTP handler 转 HTTPS upstream 都会通过 HTTP/1.1 fallback 返回完整 body。
- 按用例启动独立端口代理，创建 `htmlAppend` 规则，请求 HTML 文档并断言：
  - `htmlAppend` 注入脚本出现在 `</html>` 之前；
  - `htmlAppend` 注入脚本不会出现在 `</html>` 之后；
  - `htmlPrepend` 内容出现在 `<html>` 开始标签之后；
  - `htmlPrepend` 内容不会出现在 `<!doctype>` 或 `<html>` 之前；
  - 原始 HTML 主体内容保持存在。
- 按矩阵用例启动本地内容类型上游服务，请求三类 Content-Type 响应并逐项断言 append/prepend/body 的实际响应体。

## 校验要求

- `cargo test -p bifrost-proxy test_html_injection_append -- --nocapture`
- `cargo test -p bifrost-proxy test_html_injection_prepend -- --nocapture`
- `cargo test -p bifrost-proxy test_html_injection_gzip_preserves_encoding -- --nocapture`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_script`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_gzip_response`
- `cargo run -p bifrost-e2e -- --test body_htmlAppend_gzip_response_delete_encoding`
- `cargo run -p bifrost-e2e -- --test body_content_injection_protocol_matrix`
- `cargo run -p bifrost-e2e -- --test body_content_injection_mock_resources`
- `cargo run -p bifrost-e2e -- --test body_https_htmlAppend_forwarded_http`
- `bash e2e-tests/tests/test_badge_injection_e2e.sh`
- 按 `human_tests/proxy-rules-advanced.md` 执行 `TC-PRA-59`
- 按 `human_tests/proxy-rules-advanced.md` 执行 `TC-PRA-60`
- `cargo test -p bifrost-proxy h2_body_recovery_policy -- --nocapture`
- `cargo test -p bifrost-tests --test https_proxy_test retries_h2_body_failure -- --nocapture`
- 按 `human_tests/proxy-rules-advanced.md` 执行 `TC-PRA-25`、`TC-PRA-29A`、`TC-PRA-29B` 与 `TC-PRA-29D`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

## 文档更新要求

- 已更新 `human_tests/proxy-rules-advanced.md` 与 `human_tests/readme.md`。
- 本次未新增 CLI 参数、协议名称或公开配置项，不需要更新 `README.md`。
