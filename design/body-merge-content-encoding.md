# Body Merge Content-Encoding 设计方案

## 背景

`reqMerge://` 和 `resMerge://` 用于把 JSON patch 合并进请求或响应 Body。真实浏览器/API 请求常带 `Content-Encoding: gzip`、`br` 或 `zstd`，Body 规则必须在解压后的明文内容上执行，并在转发给上游或客户端前恢复为最终响应/请求头声明的编码，否则整个 Body 类规则（`reqBody` / `resBody` / `reqPrepend` / `reqAppend` / `reqReplace` / `reqMerge` / 对应 res 版本、HTML/JS/CSS 内容注入）都会因为对压缩字节做 JSON 解析而静默失败。

旧代理链路直接把原始 Body bytes 传给 `apply_body_rules`。当 Body 是 gzip JSON 时，`serde_json::from_slice` 读到的是压缩字节，`reqMerge` / `resMerge` 会返回 `Ok(None)` 或原样透传，用户看不到规则应该有的效果，只能通过关掉压缩绕过；这在真实 App 或浏览器场景根本不现实。

本方案定义一条通用的“按最终 `Content-Encoding` 解码 → 应用 Body 规则 → 按最终 `Content-Encoding` 编码”流水线，覆盖普通 HTTP、HTTPS MITM、mock/immediate response、脚本、内容注入、trailers 与 Replay Admin API 六条落地路径。

## 用户目标验证清单

### 必须实现

- gzip（含 `x-gzip` 兼容别名）/ deflate / brotli / zstd 请求 Body 命中 `reqBody` / `reqPrepend` / `reqAppend` / `reqReplace` / `reqMerge` 后，最终发给上游的 Body 与最终 `Content-Encoding` 保持一致；如果规则删除了 `Content-Encoding`，Body 以 identity 输出。
- gzip（含 `x-gzip` 兼容别名）/ deflate / brotli / zstd 响应 Body 命中同类响应规则后，客户端使用 `--compressed` 能读到合并/替换后的明文；如果规则删除了 `Content-Encoding`，客户端读到 identity Body。
- `Content-Encoding` 的逗号分隔编码链和重复 header 字段按线上顺序合并、按逆序完整解码；请求与响应的 Traffic detail、network `.bifrost` 导出和导入预览均展示明文。
- HTTPS 解包链路（MITM）与普通 HTTP 走同一份保编码 Body 规则代码，不允许存在两套实现导致行为分叉。
- mock、immediate response、rawfile 响应也走保编码 Body 规则和内容注入，携带压缩响应时不会绕过修复。
- `reqScript` / `resScript` 进入脚本前按当前 `Content-Encoding` 解码为文本；脚本写回 Body 后按脚本最终 Header 重新编码或输出 identity。
- HTML / JS / CSS 内容注入必须在解压后的明文上执行，并按最终 `Content-Encoding` 重新编码。
- 命中 `trailers://` 时，普通 HTTP handler、HTTPS tunnel/MITM、mock 与 immediate response 的 buffered body 分支都必须按 trailer stream 规范化响应头：保留 `Trailer` 声明、移除 `Content-Length`。
- Replay Admin API 走 replay 专属规则链路，请求/响应两侧的 Body/Header/Cookie/CORS/内容注入/脚本 / `decode://bp` 都被执行；执行结果、`req_script_results` / `res_script_results` 与解码后的 Body 写入 Traffic detail。

### 必须不破坏

- Body 规则不命中的请求走原有零拷贝快路径，不做无谓的解压/再压缩。
- 未知、自定义、加密或依赖外部字典的 `Content-Encoding` 与 identity Body 保持原字节，不引入损坏的压缩输出，并继续留给自定义 decoder 处理。
- 解压失败（例如上游头声明 gzip 但 Body 实际是 identity）时保持原始 Body 与原编码，避免生成损坏响应。
- `delete://reqHeaders.Content-Encoding` 或 `reqHeaders://(Content-Encoding: gzip)` 修改最终编码时，Body 与最终 `Content-Encoding` 必须保持一致，不允许头声明与实际编码错位。
- WebSocket / 二进制 tunnel / SSE stream 路径不因新增保编码链路引入强制 body collect。
- Replay Admin API 返回结构、Traffic detail 现有字段保持向后兼容。
- Body 规则不命中或没有 `Content-Encoding` 的路径，`Cargo.toml` 不新增强制的解压依赖以外行为。

### 必须真实验证

- 单元测试覆盖 gzip 请求/响应 `reqMerge` / `resMerge`、identity 输出、脚本双向编解码、内容注入保编码。
- E2E 覆盖普通 HTTP 与 HTTPS MITM 两种链路下的 gzip Body `reqMerge` / `resMerge`。
- Replay E2E 覆盖 request/response custom rules 全矩阵、Replay script、Replay `decode://bp`。
- 真实场景使用真实业务 API（例如 `page_permission` 或其他 gzip JSON 接口）验证 `resMerge://({"test":"qwe"})` 命中后，最终 JSON 顶层包含 `"test":"qwe"`。
- 命中 `trailers://` 的 buffered 响应真实回归：`Content-Length` 必须消失，`Trailer` 声明必须保留。

## 产品语义

### “Body 规则总是在明文上执行”

对用户而言，Body 规则永远在明文 JSON / 文本上执行，与请求/响应最终使用什么编码无关。用户可以直接写：

```txt
api.example.com resMerge://({"test":"qwe"})
```

不需要再去思考“上游是不是 gzip”“客户端是否支持 br”。Bifrost 内部先按当前 `Content-Encoding` 解压 → 应用规则 → 按最终 `Content-Encoding` 重新压缩；客户端 / 上游看到的 Body 编码与它们的头声明始终一致。

### “头修改与 Body 编码必须一致”

如果 Header 类规则改动了 `Content-Encoding`（例如 `delete://resHeaders.Content-Encoding` 强制走 identity，或 `resHeaders://(Content-Encoding: gzip)` 强制转 gzip），Body 输出必须按最终 header 重新编码。头声明与实际编码错位是 Bifrost 视角下的“损坏响应”，任何路径都不允许出现。

### “标准自包含压缩默认解码，未识别编码保持透传”

对 gzip / `x-gzip` / deflate / br / zstd 以及 identity，Bifrost 默认支持单层和组合链；多个 coding 按声明顺序编码、按逆序解码，重复的 `Content-Encoding` header 字段先按线上顺序合并。对于未知、自定义、加密、专用格式或需要协商字典才能恢复的编码，保编码流水线不猜测、不执行，直接保留原 Body 与原编码，交给用户配置的自定义 decoder，避免生成任何损坏输出。

## 技术细节

### 核心函数

`crates/bifrost-proxy/src/transform/body.rs`：

- `apply_body_rules(body, rules, phase, content_type, verbose_logging, ctx) -> Bytes`
  已有的明文 Body 规则执行入口，仅认识 identity Body。
- `apply_body_rules_preserving_encoding(body, rules, phase, source_encoding, final_encoding, content_type, verbose_logging, ctx) -> Bytes`
  新增。按 `source_encoding` 解压 → 调 `apply_body_rules` → 按 `final_encoding` 编码；`source_encoding == final_encoding` 且为 identity 时短路直接调 `apply_body_rules`。解压失败保留原字节与原编码。
- `apply_content_injection_preserving_encoding(body, injection_rules, source_encoding, final_encoding, content_type, ctx) -> Bytes`
  新增。HTML/JS/CSS 注入的保编码封装。

`crates/bifrost-proxy/src/transform/compress.rs` / `decompress.rs`：负责 gzip / `x-gzip` / deflate / br / zstd / identity 的组合编解码。编码按 header 声明顺序执行，解码按逆序执行；任一环节未知或失败时整条链回退原字节，供保编码链路和自定义 decoder 做 fallback 判断。

### Traffic 抓取与展示

- `request_body_ref` / `response_body_ref` 是唯一的 canonical wire bytes；普通缓冲 Body、流式 Body 和大文件 Body 都不在代理热路径生成第二份明文副本。
- 实际 `Content-Encoding` 写入 Traffic detail 的版本化 `body_metadata_blob`，与 Body 引用在同一 SQLite 事务中提交。v14 的 `.content-encoding` sidecar 只做读取兼容，新流量不再创建 sidecar。
- `traffic get`、Traffic Body API、批量 Body API、全文搜索、JSONPath Body 条件过滤、搜索结果 `include`、SSE 事件恢复和 Network 导出统一使用“metadata 优先、旧 sidecar 回退”的 logical-body loader，避免直接把 wire bytes 当 UTF-8，也避免对已经是明文的旧记录二次解码。
- `raw=1` 优先读取 decode/script 流程显式保留的 raw 引用；普通代理流量没有独立 raw 引用时直接回退 canonical wire ref，因此 raw 恢复不需要双份落盘。
- 展示与导出的完整编码链共享 10 MiB 解压输出预算，不会让每层单独重复消耗 10 MiB；超限、损坏或未知编码保留原始落盘引用，不伪造明文。Traffic API 的 `raw=1` 优先读取独立 raw 引用；不存在 raw 引用时沿用既有的 body 引用回退语义。
- Traffic Body 读取使用当前 `sandbox.limits.max_decompress_output_bytes`，没有配置管理器时才回退到 10 MiB 默认值。
- gzip 使用多 member 解码语义，合法的相邻 gzip member 会在同一 10 MiB 预算内全部展开并顺序拼接。
- network `.bifrost` 导出保留 wire bytes 的 base64，同时写入供人查看的明文；导入预览优先展示明文，旧版本已做 lossy UTF-8 转换的不可逆数据给出明确警告。
- 确认导入 Network 包时，有 base64 就把它作为 canonical wire body 写入主引用并把编码链写入 DB metadata；只有没有 base64 的旧包才回退写入明文。导入阶段不解压、不创建 raw 副本，也不消耗展示/搜索的解压预算；非法 base64 或超长 encoding metadata 在写入任何记录前整体拒绝。

### SSE 热路径

- identity SSE 只做有界换行扫描；压缩 SSE 的增量解码和事件计数由有界后台 observer 完成。
- `poll_frame` 对压缩 SSE 只执行 `Bytes::clone()` 和 `try_send`，不等待队列、不执行解压、flush 或正文扫描。observer 队列满或解码失败时立即停止观察并把 `observation_partial` 写入 Traffic metadata，转发和 wire 落盘继续进行。
- Super Performance Mode 不创建 decoder/observer。SSE 结束后的 OpenAI-like 派生正文也在 blocking worker 中生成，结束帧不再同步整文件读取和重解压。

### HTTP / HTTPS 落地

`crates/bifrost-proxy/src/proxy/http/handler.rs`：

- `apply_req_rules` 完成后取最终请求 `Content-Encoding`，调用 `apply_body_rules_preserving_encoding` 处理请求 Body。
- `apply_res_rules` 完成后取最终响应 `Content-Encoding`，先跑 Body 规则再跑内容注入，两者共用同一份最终 `Content-Encoding`。
- 命中 `trailers://` 的 buffered body 分支：移除 `Content-Length`、保留 `Trailer` 声明，避免最终 body 附带 trailers 但固定长度头被重新写回。

`crates/bifrost-proxy/src/proxy/http/tunnel/mod.rs`：HTTPS MITM 使用同一份保编码 Body 规则与内容注入。HTTPS path 级 Body 规则要求先命中 `tlsIntercept://`；CONNECT 阶段没有 path，因此 relay / 远端代理验证时必须同时配置目标 host 的 `tlsIntercept://`，或通过启动配置把目标 host 纳入 TLS 拦截范围。

### 脚本 (reqScript / resScript)

进入脚本前：按 `Content-Encoding` 解码为文本 (UTF-8 优先，失败回退 base64)。脚本执行完成后：读取脚本可能改动过的 Header，按最终 `Content-Encoding` 重新编码 Body；如果脚本清空/删除 `Content-Encoding`，Body 以 identity 输出。

### Mock / Immediate Response / Rawfile

`status-code-rule-pipeline` / `mock` / `rawfile` 分支同样调用 `apply_body_rules_preserving_encoding` 与 `apply_content_injection_preserving_encoding`，避免仅普通代理路径生效。命中 `trailers://` 时按同一 trailer 规范化响应头。

### Replay Admin API

Replay 不复用普通代理的 transform 管线，需要在 replay 专属路径补齐规则应用：

请求侧（`crates/bifrost-admin/src/request_rules.rs` + `replay_body_rules.rs`）：

- `reqHeaders` / `reqCookies` / `delete://reqHeaders.*` / `delete://urlParams.*` / `urlParams` / `urlReplace`。
- `reqBody` / `reqPrepend` / `reqAppend` / `reqReplace` / `reqMerge` / `reqType` / `reqCharset`。
- `forwardedFor` / `headerReplace://req.*` / `reqCors`。

响应侧（`crates/bifrost-admin/src/replay_response_rules.rs`）：

- `resHeaders` / `delete://resHeaders.*` / `statusCode` / `replaceStatus` / `resCookies` / `resCors` / `resType` / `resCharset` / `cache` / `attachment` / `responseFor` / `trailers` / `headerReplace://res.*`。
- `resBody` / `resPrepend` / `resAppend` / `resReplace` / `resMerge`，HTML/JS/CSS 内容注入。

脚本 / 解码：

- `reqScript` / `resScript` 通过 `crates/bifrost-admin/src/replay_scripts.rs` 执行并把执行结果写入 Replay 生成的 Traffic 详情。
- `decode://...` 与 `decode://bp` 作为落库前解码链路执行；解码后的请求/响应 Body 写入 Traffic body 视图，原始 Body 写入 raw body 引用。

Replay 不复现 `reqDelay` / `resDelay` / `reqSpeed` / `resSpeed` 这类传输时序控制（Replay Admin API 返回的是执行结果 JSON，不是真实代理传输）。

## CLI 交互

Body 规则本身没有新增 CLI 命令，但下列 CLI 场景需要行为一致：

- `bifrost rule syntax-check` 对 `reqMerge://` / `resMerge://` 的 JSON 片段做基础校验，非法 JSON 输出可诊断错误。
- `bifrost traffic get <id>` 与 `bifrost search` 在展示 Body 时读取解码后的 `request_body_ref` / `response_body_ref`（普通代理），或读取 Replay Admin API 写入的解码 Body（Replay Traffic）。

## Web / Admin API

- Rules 编辑器的 syntax hint 对 `reqMerge://` / `resMerge://` 后紧跟的 JSON 片段做补全提示，鼓励用户写完整 JSON 对象；无格式约束的第一版不做阻断。
- Replay Admin API 返回的 `data.body` 字段是执行完保编码 Body 规则后的最终结果；`raw_body_ref` 保留上游原始 Body 引用。
- Traffic detail 的响应 body tab 展示解码后的 Body，供搜索和 diff；raw body tab 展示上游最终 Body（保留原编码）。

## Sync 边界

- 保编码链路是代理运行时行为，不产生跨设备状态，不参与 Sync。
- Replay 的规则应用结果（`req_script_results` / `res_script_results` / decoded body）是本地 Traffic detail 的一部分，不主动 sync。

## 实现切分

### Phase 1：核心函数与普通 HTTP 落地

- 在 `body.rs` 新增 `apply_body_rules_preserving_encoding` 与 `apply_content_injection_preserving_encoding`，附带 gzip / identity / 未知编码单元测试。
- `handler.rs` 请求侧接入保编码 Body 规则，响应侧接入保编码 Body 规则 + 内容注入。
- 请求头 `Content-Encoding` 修改与实际 Body 编码保持一致。
- Trailers 分支：命中 `trailers://` 的 buffered body 移除 `Content-Length`，保留 `Trailer` 声明。

### Phase 2：HTTPS MITM / Mock / Rawfile 对齐

- `tunnel/mod.rs` 请求/响应两侧接入同一份保编码 Body 规则与内容注入。
- Mock、immediate response、rawfile 分支复用保编码 Body 规则；trailer 头规范化对齐。
- Handler 与 tunnel 用同一份实现，避免行为漂移。

### Phase 3：脚本双向编解码

- `reqScript` / `resScript` 前置解码 + 后置按最终 header 编码。
- 脚本删除 `Content-Encoding` 时输出 identity。
- 单元测试覆盖脚本读写 gzip Body 的双向路径。

### Phase 4：Replay Admin API 与 human_tests

- Replay 请求 / 响应 / 脚本 / decode 全矩阵接入；Replay Traffic body / raw body 一致写入。
- 更新 `human_tests/proxy-rules-advanced.md` 与 `human_tests/api-replay.md`，新增压缩 JSON 的 `reqMerge` / `resMerge` 回归、Replay 全矩阵回归、Replay 脚本 / `decode://bp` 回归。
- 更新 `human_tests/readme.md` 用例数量。

## 测试方案

### 单元测试

- `body::apply_body_rules_preserving_encoding_gzip_reqMerge`：gzip 请求 JSON 经过 `reqMerge` 后，解压结果包含新增字段并覆盖相同 key，输出仍是 gzip。
- `body::apply_body_rules_preserving_encoding_gzip_resMerge`：gzip 响应 JSON 经过 `resMerge` 后，解压结果包含新增字段并覆盖相同 key，输出仍是 gzip。
- `body::apply_body_rules_preserving_encoding_gzip_to_identity`：gzip 响应 JSON 同时删除最终 `Content-Encoding` 后，输出为 identity JSON。
- `body::apply_body_rules_preserving_encoding_unknown_encoding_passthrough`：未识别编码保持原 Body 与原编码。
- `decompress::multiple_content_codings`：重复 header / 逗号链按逆序完整解码，并覆盖 `x-gzip` 别名。
- `network_body::repeated_content_encoding_headers_and_x_gzip_are_decoded`：Network 包内字节按重复编码 header 解码；未知编码保持原字节。
- `decompress::test_multiple_content_codings_share_one_output_budget`：多层编码共享同一解压预算，避免按层重复分配上限。
- `traffic::stored_body_tests::only_decodes_refs_marked_as_content_encoded`：Traffic 读取只解码带持久化编码标记的 wire body，已经移除 HTTP 外层编码的 `application/gzip` 数据不会被二次解码。
- `query_service::traffic_get_decodes_content_encoded_file_body`：CLI/Remote Invoke 共用的 `traffic get` 查询服务返回解码后的文件型 body。
- `search::encoded_file_body_is_decoded_for_search_json_filter_and_include`：关键词搜索、JSONPath 条件与 include body 同时读取解码后的文件型 body。
- `traffic::batch_query_tests::batch_body_chunk_decodes_content_encoded_references`：批量 Body API 在截断、计算大小和 base64 编码前先解码带标记的请求/响应引用。
- `traffic::sse_stream_tests::content_encoded_sse_body_is_decoded_before_event_parsing`：SSE 事件解析器只消费完成 HTTP 解码后的字节。
- `traffic::stored_body_tests::configured_decompression_limit_is_honored_by_body_reads`：Traffic Body 读取遵循运行时配置的解压上限，超限时保留 wire bytes。
- `network_body::concatenated_gzip_members_are_all_decoded` / `decompress::test_decompresses_all_concatenated_gzip_members`：管理端与代理端完整解码相邻 gzip member。
- `bifrost_file::imported_network_bodies_persist_plaintext_and_raw_bytes`：Network 导入后主 Body 与 raw Body 都可继续读取。
- `bifrost_file::malformed_lossless_body_fields_are_rejected`：请求和响应的非法 lossless base64 在预览/导入前返回校验错误，不静默丢弃。
- `bifrost_file::multi_record_preview_does_not_decompress_lossless_body_fields`：多记录摘要只检测旧 lossy 文本，不批量展开压缩 Body。
- `body_metadata::only_identity_tokens_are_classified_as_unencoded`：重复 `identity` 字段仍视为无编码，混合标准或自定义编码不会误判。
- `body::apply_body_rules_preserving_encoding_decode_failure_passthrough`：头声明 gzip 但 Body 实际是 identity，解压失败保留原字节。
- `body::apply_content_injection_preserving_encoding_gzip_html`：gzip HTML 注入 badge/inline script 后仍是有效 gzip 且解码后 HTML 结构正确。
- `scripts::script_gzip_roundtrip`：gzip Body 进入脚本前会解码为文本，脚本写回 Body 后仍可按 gzip 重新编码。
- `handler::mock_gzip_resMerge_preserved`：mock / immediate gzip JSON 响应经过 `resMerge` 后仍保持有效 gzip。
- `handler::trailers_buffered_removes_content_length`：buffered 响应命中 `trailers://` 后，最终 header normalization 不会重新写回 `Content-Length`，并保留 `Trailer` 声明头。
- `replay_body_rules::resMerge_gzip_body_returns_merged_json`：Replay 响应 JSON 经过 `resMerge` 后，`data.body` 里的 JSON 包含新增字段并覆盖相同 key。
- `request_rules::replay_request_full_matrix`：`reqMerge`、URL 参数删除、请求头替换、Content-Type/charset、CORS 预检头与 forwarded-for 都体现在发给上游的请求中。
- `replay_response_rules::replay_response_full_matrix`：响应头、状态码、cookie、CORS、Content-Type/charset、缓存、附件、responseFor、trailers、响应头替换、内容注入与 Body 修改都体现在 Admin API 的返回数据中。
- `replay_scripts::script_results_recorded`：Replay `reqScript` / `resScript` 修改后的请求与响应会真实生效，Traffic detail 会记录 `req_script_results` / `res_script_results`。
- `replay_scripts::decode_bp_records_parser_output`：Replay `decode://bp` 会执行绑定的 `bp://` parser，并在 Traffic detail/body 中记录 request/response phase 的 parser 输出。

### E2E 测试

- `e2e-tests/tests/test_body_reqmerge_gzip_json.sh`：curl 发送 gzip JSON 请求，代理执行 `reqMerge`，上游收到仍可解压的 gzip JSON。
- `e2e-tests/tests/test_body_resmerge_gzip_json.sh`：上游返回 gzip JSON，代理执行 `resMerge`，客户端用 `--compressed` 读到合并后的 JSON，响应头仍是 gzip。
- `e2e-tests/tests/test_body_https_reqmerge_gzip_json.sh`：HTTPS 解包转发到 HTTP 上游时，gzip JSON 请求经过 `reqMerge` 后仍保持有效 gzip。
- `e2e-tests/tests/test_body_https_resmerge_gzip_json.sh`：HTTPS 解包转发到 HTTP 上游时，gzip JSON 响应经过 `resMerge` 后仍保持有效 gzip。
- `e2e-tests/tests/test_replay_rules.sh`：本地 echo/SSE/WebSocket 上游验证 Replay custom rules，`request_body_mutations.txt` 覆盖 `reqPrepend` / `reqAppend` / `reqReplace`，`full_modify_matrix.txt` 覆盖 replay 请求修改、响应 metadata、响应 Body 修改和内容注入规则矩阵，`req_res_script.txt` 覆盖 Replay 的 Request/Response Script，`bp_decode.txt` 覆盖 Replay Traffic 落库前的 `decode://bp`。
- `e2e-tests/tests/test_temporary_port_bindings.sh`：真实代理记录双层编码、多 member gzip 请求/响应和 gzip SSE，验证 Traffic API、`traffic get`、批量 Body API、搜索关键词、响应 JSONPath 过滤、include body、SSE 事件恢复、Network 导出/预览/导入均返回明文，同时 raw body 仍可恢复 wire bytes。
- `e2e-tests/tests/test_response_stream_script.sh`：HTTP 与 HTTPS MITM 上游返回两个 `Content-Encoding: identity` 时，`resStreamScript` 仍正常逐事件转换；真正的 gzip 编码继续明确拒绝。

### 真实场景测试 human_tests

新增 / 更新：

- `human_tests/proxy-rules-advanced.md` 新增压缩 JSON 的 `reqMerge` / `resMerge` 回归用例（普通 HTTP 与 HTTPS MITM 两条链路各一条），按文档真实执行。
- `human_tests/api-replay.md` 新增 Replay Admin API 的 `resMerge` 响应 Body 回归、replay 规则覆盖回归、Replay 脚本 / BP Decode 回归用例，并用临时代理端口真实执行。
- 使用真实目标 `page_permission` 接口验证 `resMerge://({"test":"qwe"})` 命中后，最终 JSON 顶层包含 `"test":"qwe"`；验证规则需包含目标 host 的 TLS 解包前置规则。
- 更新 `human_tests/readme.md` 用例数量。

所有服务启动必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-proxy transform::body`
- `cargo test -p bifrost-admin replay_body_rules replay_response_rules request_rules replay_scripts`
- 新增的 4 条 E2E 与 `test_replay_rules.sh`
- `cargo test --workspace --all-features`
- `rust-project-validate`

本机有 no-local-coverage 约定时，不运行 `make coverage` / `make coverage-unit`；交付说明 coverage 本地豁免，并依赖其他本地验证与远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：gzip/br/zstd 请求/响应 `reqMerge` / `resMerge` 明文可见 → 编码保留 → 头与编码一致；HTTPS MITM 与普通 HTTP 一致；Replay 全矩阵；trailer 规范化。
- 复核 diff：`body.rs` / `handler.rs` / `tunnel/mod.rs` / mock / rawfile / script / replay 六条路径是否都调用保编码入口。
- 重点 review：默认路径是否引入不必要的解压；未知 `Content-Encoding` 是否被误当 identity；trailer 分支是否重新写回 `Content-Length`。
- 复测：focused 单元测试、4 条新增 E2E、`test_replay_rules.sh`、human_tests 真实执行。

### 第 2 轮

- 复核第 1 轮发现问题的修复。
- 再次检查 `git status --short`、`git diff`、新增文件与 human_tests 索引。
- 重点 review：Replay Admin API 返回的 body 是否与执行链路一致；`req_script_results` / `res_script_results` 是否写入 Traffic detail；`decode://bp` 是否与普通代理 decode 路径共用同一份 parser 输出格式。
- 复测：失败路径重跑、`cargo test --workspace --all-features`、`rust-project-validate`。

## 风险与决策点

- 未识别编码策略：本方案选择“透传 + warning”，不尝试猜测。组合 `Content-Encoding` 已支持，但组合链中只要包含未知、自定义、加密、专用格式或依赖外部字典的 coding，就整链保留原字节并交给自定义 decoder。
- 文件型 body 保持代理热路径原样落盘，并把编码链和引用一起持久化，Traffic / Network 读取时才有界解压；这消除了异步更新 Traffic 引用与最终记录写入之间的竞态，也避免仅凭 header 猜测导致双层 gzip 被多解一层。超出安全解压上限时仍返回原字节，这是防压缩炸弹边界，不应通过取消上限绕过。
- 保编码链路是否引入拷贝开销：所有非命中 Body 规则的请求走 identity 短路，不做无谓的解压/再压缩；命中规则时的一次解压 + 一次压缩是必要开销。
- Replay 与普通代理保编码链路必须共用 `apply_body_rules_preserving_encoding`，避免两套实现漂移；如果 Replay 出现新的规则类型，必须先补 replay 侧再上普通代理，否则会出现“Replay 有效但真实代理无效”的产品倒挂。
- HTTPS path 级 Body 规则要求先命中 `tlsIntercept://`；文档、CLI 提示与规则编辑器 hint 需要同步说明这一前置条件，避免用户以为规则未生效而反复调试。
