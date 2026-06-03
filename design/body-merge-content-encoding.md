# Body Merge Content-Encoding

## 功能模块

`reqMerge://` 和 `resMerge://` 用于把 JSON patch 合并进请求或响应 Body。真实浏览器/API 请求常带 `Content-Encoding: gzip`、`br` 或 `zstd`，Body 规则必须在解压后的明文内容上执行，并在转发给上游或客户端前恢复为最终响应/请求头声明的编码。

## 问题与实现逻辑

- 旧链路直接把原始 Body bytes 传给 `apply_body_rules`。当 Body 是 gzip JSON 时，`serde_json::from_slice` 读到的是压缩字节，`reqMerge` / `resMerge` 会静默跳过。
- 新增 `apply_body_rules_preserving_encoding`：
  - 按源 `Content-Encoding` 解压 Body。
  - 在解压后的明文上执行 `reqBody` / `resBody`、prepend、append、replace、merge 等 Body 规则。
  - 按规则修改后的最终 `Content-Encoding` 重新压缩；如果最终编码被删除，则输出 identity Body。
  - 解压失败时保持原始 Body 与原编码，避免生成损坏响应。
- HTTP 请求侧在 `apply_req_rules` 后读取最终请求 `Content-Encoding`，让 `delete://reqHeaders.Content-Encoding` 或 `reqHeaders://(Content-Encoding: gzip)` 与 Body 实际编码保持一致。
- HTTP 响应侧在 `apply_res_rules` 后读取最终响应 `Content-Encoding`，先执行 Body 规则保编码，再执行 HTML/JS/CSS 内容注入保编码。
- HTTPS tunnel/MITM 链路同样使用上述保编码 Body 规则处理，避免仅普通 HTTP 代理路径生效。
- HTTPS path 级 Body 规则只能在 TLS 解包后看到路径和响应体；CONNECT 阶段没有 path，所以 relay / 远端代理验证时必须同时配置目标 host 的 `tlsIntercept://`，或通过启动配置把目标 host 纳入 TLS 拦截范围。
- `reqScript` / `resScript` 进入脚本前按当前 `Content-Encoding` 解码为文本；脚本写回 Body 后，再按脚本最终 Header 重新编码或输出 identity。
- mock / immediate response 路径同样使用保编码 Body 规则和内容注入，避免 rawfile / 远端 mock 携带压缩响应时绕过修复。
- Replay Admin API 不复用普通代理的完整 transform 管线，必须在 replay 专属路径补齐规则应用：
  - 请求侧执行 `reqHeaders` / `reqCookies` / `delete://reqHeaders.*` / `delete://urlParams.*` / `urlParams` / `urlReplace` / `reqBody` / `reqPrepend` / `reqAppend` / `reqReplace` / `reqMerge` / `reqType` / `reqCharset` / `forwardedFor` / `headerReplace://req.*` / `reqCors`。
  - 响应侧执行 `resHeaders` / `delete://resHeaders.*` / `statusCode` / `replaceStatus` / `resCookies` / `resCors` / `resType` / `resCharset` / `cache` / `attachment` / `responseFor` / `trailers` / `headerReplace://res.*` / `resBody` / `resPrepend` / `resAppend` / `resReplace` / `resMerge`，以及 HTML/JS/CSS 内容注入规则。
  - 脚本侧执行 `reqScript` / `resScript`，并把执行结果写入 Replay 生成的 Traffic 详情；`decode://...` 与 `decode://bp` 作为落库前解码链路执行，解码后的请求/响应 Body 写入 Traffic body 视图，原始 Body 写入 raw body 引用。
  - `reqDelay` / `resDelay` / `reqSpeed` / `resSpeed` 属于真实代理传输时序控制；Replay Admin API 返回的是执行结果 JSON，不做传输节流语义复现。

## 依赖项

- `crates/bifrost-proxy/src/transform/compress.rs`
- `crates/bifrost-proxy/src/transform/decompress.rs`
- `crates/bifrost-proxy/src/proxy/http/handler.rs`

## 测试方案

- 单元测试：
  - gzip 请求 JSON 经过 `reqMerge` 后，解压结果包含新增字段并覆盖相同 key，输出仍是 gzip。
  - gzip 响应 JSON 经过 `resMerge` 后，解压结果包含新增字段并覆盖相同 key，输出仍是 gzip。
  - gzip 响应 JSON 同时删除最终 `Content-Encoding` 后，输出为 identity JSON。
  - gzip body 进入脚本前会解码为文本，脚本写回 body 后仍可按 gzip 重新编码。
  - mock / immediate gzip JSON 响应经过 `resMerge` 后仍保持有效 gzip。
  - Replay 响应 JSON 经过 `resMerge` 后，`data.body` 里的 JSON 包含新增字段并覆盖相同 key。
  - Replay request 侧 `reqMerge`、URL 参数删除、请求头替换、Content-Type/charset、CORS 预检头与 forwarded-for 都体现在发给上游的请求中。
  - Replay response 侧响应头、状态码、cookie、CORS、Content-Type/charset、缓存、附件、responseFor、trailers、响应头替换、内容注入与 Body 修改都体现在 Admin API 的返回数据中。
  - Replay `reqScript` / `resScript` 修改后的请求与响应会真实生效，Traffic detail 会记录 `req_script_results` / `res_script_results`。
  - Replay `decode://bp` 会执行绑定的 `bp://` parser，并在 Traffic detail/body 中记录 request/response phase 的 parser 输出。
- E2E 测试：
  - `body_reqMerge_gzip_json`：curl 发送 gzip JSON 请求，代理执行 `reqMerge`，上游收到仍可解压的 gzip JSON。
  - `body_resMerge_gzip_json`：上游返回 gzip JSON，代理执行 `resMerge`，客户端用 `--compressed` 读到合并后的 JSON，响应头仍是 gzip。
  - `body_https_reqMerge_gzip_json`：HTTPS 解包转发到 HTTP 上游时，gzip JSON 请求经过 `reqMerge` 后仍保持有效 gzip。
  - `body_https_resMerge_gzip_json`：HTTPS 解包转发到 HTTP 上游时，gzip JSON 响应经过 `resMerge` 后仍保持有效 gzip。
  - `e2e-tests/tests/test_replay_rules.sh`：使用本地 echo/SSE/WebSocket 上游验证 Replay custom rules，其中 `request_body_mutations.txt` 覆盖 `reqPrepend` / `reqAppend` / `reqReplace`，`full_modify_matrix.txt` 覆盖 replay 请求修改、响应 metadata、响应 Body 修改和内容注入规则矩阵，`req_res_script.txt` 覆盖 Replay 的 Request/Response Script，`bp_decode.txt` 覆盖 Replay Traffic 落库前的 `decode://bp`。
- 真实场景测试：
  - 更新 `human_tests/proxy-rules-advanced.md`，新增压缩 JSON 的 `reqMerge` / `resMerge` 回归用例，并按文档真实执行。
  - 更新 `human_tests/api-replay.md`，新增 Replay Admin API 的 `resMerge` 响应 Body 回归用例、replay 规则覆盖回归用例和 Replay 脚本/BPDecode 回归用例，并用临时代理端口真实执行。
  - 使用真实目标 `page_permission` 接口验证 `resMerge://({"test":"qwe"})` 命中后，最终 JSON 顶层包含 `"test":"qwe"`；验证规则需包含目标 host 的 TLS 解包前置规则。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、当前 diff、请求/响应编码一致性；运行 focused 单元测试与两个新增 E2E。
- 第 2 轮：复查第 1 轮后的最终 diff、human_tests 文档与索引；复跑 affected E2E 与 `cargo test -p bifrost-proxy transform::body`。

## 校验要求

- 必须执行 focused Rust 单元测试、两个新增 bifrost-e2e 用例、human_tests 真实命令。
- 收尾前执行 `cargo test --workspace --all-features` 和 rust-project-validate；若因环境阻塞，必须记录具体失败点。

## 文档更新要求

- 本设计文档记录实现语义。
- `human_tests/proxy-rules-advanced.md` 增加真实回归用例。
- `human_tests/readme.md` 同步用例数量。
