# bp protocol parser

## 背景

部分二进制协议解析工具会把请求/响应解析成文本并替换 stream body。Bifrost 默认代理语义不能直接采用这种改写行为，否则真实客户端可能收到不再兼容业务协议的 JSON。

本方案将 `bp://` 定义为二进制协议解析脚本引用，默认只配合 `decode://bp` 进入 Traffic 存储、搜索和详情展示，不改写真实上游/下游流量。

## 规则语义

```txt
<pattern> bp://<local-parser-name> decode://bp
<pattern> bp://<local-parser-name?parser-options> decode://bp
<pattern> bp://<https-remote-parser-url?sha256=<hex>> decode://bp
```

- `bp://<name>`：加载本地 `scripts/parser/<name>.js`。
- `bp://<name?parser-options>`：加载本地 `scripts/parser/<name>.js`，完整引用保留在 `ctx.scriptName`，脚本可自行解析 `idl`、`psm`、`service`、`method` 等参数。
- `bp://<https-url?...>`：下载远程 parser 脚本，校验 sha256 后缓存到 `scripts/_remote-cache/parser/`。
- `decode://bp`：执行当前命中的 bp parser，输出用于落库展示和搜索。
- 未配置 `decode://bp` 时，`bp://` 只作为匹配到的规则元数据，不触发解析。

不支持 `bp://script:`、`bp://thrift:`、`bp://protobuf:`、`bp://cmd:`、`bp://wasm:` 这类 provider 前缀。Rust 层只认识 parser 脚本，不认识具体业务协议。

## IDL / PSM 配置方式

用户侧保持 `bp://` 只表达“使用一个 parser 脚本”，业务协议输入作为脚本参数：

```txt
api.example.com bp://build_in_bp?idl=file:///Users/eden/work/code/nextoncall/next_agent/idl/order.thrift&service=OrderService&method=GetOrder
api.example.com decode://bp

api.example.com bp://build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
api.example.com decode://bp
```

长配置可通过 value profile 收敛，让规则更容易理解：

````txt
api.example.com bp://{order_bp}
api.example.com decode://bp

```order_bp
build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
```
````

Rust 层只负责保留 `bp://` 脚本引用语义：远程 URL 不在规则 resolver 中提前下载，`bp://{profile}` 可以展开成本地脚本配置字符串，本地脚本加载时只用 `?`/`#` 前的脚本名。IDL 文件读取、PSM 到 BAM 查询、协议转换与版本缓存都由 parser JS/adapter 处理；如果后续确认 QuickJS 沙箱无法直接访问 BAM，则补一个通用 asset resolver helper，仍不在 Rust 内置 thrift/protobuf/BAM 分支。

`build_in_bp` 是推荐的 BP adapter 脚本名。仓库保留 `assets/scripts/parser/build_in_bp.js` 作为内置 parser 资源；Bifrost 启动时会自动释放到当前数据目录的 `scripts/parser/build_in_bp.js`，并在升级/启动后覆盖为随版本发布的最新内容。这样默认规则可以直接写 `bp://build_in_bp?... decode://bp`。如果用户要保留本地修改，应使用其他 parser 脚本名，避免被内置资源覆盖。

规则编辑器的智能提示需要覆盖这条成对语义：

- `bp://` 协议值提示默认给出 `build_in_bp?psm=<psm>&method=<method> decode://bp`。
- 已存在的 parser 脚本提示为 `bp://<parser> decode://bp`，确保落库解析协议一并生成。
- `decode://` 协议值提示包含 `bp`，用于用户分行书写时补齐 `decode://bp`。

### BAM token 获取

`build_in_bp` 不要求用户手工复制 BAM 页面 Cookie。脚本的 token 优先级如下：

1. `bamToken`：直接传入 BAM 可用的 Cookie，例如 `ak=bifrost_v4;c_token=...`。
2. `bamTokenFile`：读取文件中的 `bam_token` / `bamToken`，或纯文本 Cookie。
3. `syncToken` / `bifrostToken`：调用 Bifrost 同步服务 `GET /v4/sso/info`，使用 `x-bifrost-token` 换取 `data.bam_token`。
4. `syncTokenFile`：读取文件中的 `token` / `syncToken` / `sync_token`，或纯文本 sync token，再执行第 3 步。
5. 默认尝试读取 `db/config.json`，如果其中有 `bam_token` / `bamToken` 则直接使用；如果其中有 `token`，则用默认同步鉴权信息接口换取 `data.bam_token`。

服务端同步信息接口负责把已登录用户信息转换为 parser 可用的 `data.bam_token`，脚本只读取该字段，不在规则中保存 Cookie。

`build_in_bp` 还保留 `bamAuthUrl` / `bifrostInfoUrl` / `bifrostBaseUrl` 参数，便于 E2E 或私有部署环境覆盖默认同步服务地址。

### BAM 元数据接口

根据 BAM Open API 文档《API 元数据 Open API》，`build_in_bp` 解析 Thrift/Kitex RPC 时使用这些只读接口：

- `/api/endpoint/list`：通过 `psm + version/branch` 获取 endpoint 列表，并按 `rpc_method` / `path` 找到 `endpoint_id`。
- `/api/endpoint/info?schema=ref`：获取目标接口的 `req_type` / `resp_type`，用于构造 `Healthz_args` / `Healthz_result` 这类 RPC wrapper。
- `/api/service/refschema?raw_field=1`：获取保留 RPC 原字段名和 field id 的 refschema，用于按 Thrift binary field id 解包。
- `/api/service/idl`：作为排查和离线调试入口，用于确认 PSM 对应的原始 Thrift IDL 与版本。
- `/api/endpoint/info/example_code?custom_protocol=rpc`：用于生成请求/响应样例，辅助确认期望字段。

部分 parse 入口只支持 protobuf IDL，因此 `build_in_bp` 对 Thrift/Kitex 走 JS 内置 Thrift binary decoder，而不是继续依赖该类 parse 入口。

### 原始 TCP RPC 边界

Bifrost 当前 SOCKS5 / HTTP CONNECT 原始 tunnel 只落连接记录、`request_size` 和 `response_size`，不落原始 TCP payload，也不会触发 `decode://bp`。因此真实 Kitex TCP 流量如果只是作为 tunnel 经过 Bifrost，当前不能在 Traffic 详情或 Search 中展示解码后的 RPC 字段。

本期验证真实 RPC bytes 的方式是：使用真实 Kitex/Thrift binary frame 作为 HTTP body 进入现有 body capture 链路，验证 `build_in_bp` 解包、Traffic 详情展示和 `bifrost search` 搜索解码结果。后续若要支持原始 TCP tunnel 内的 RPC 解码，需要新增通用 TCP payload capture/decode 能力，且仍应把协议解析放在 parser 脚本中。

## 脚本运行时

新增 `ScriptType::Parser` 与目录：

```txt
scripts/
  parser/
  _remote-cache/
    parser/
```

Parser 脚本复用 decode 脚本输出合同：

```js
ctx.output = {
  code: "0",
  data: JSON.stringify({ ok: true }),
  msg: ""
};
```

脚本输入沿用 decode 阶段对象，并新增完整二进制输入字段：

- request 阶段：`request.bodyBase64`
- response 阶段：`response.bodyBase64`
- 仍保留 `body`、`bodyHex`、`bodySize`、`bodyHexTruncated`、`bodyTextTruncated`

这样业务 parser 可以自行决定 thrift/protobuf/BAM/envelope 解析方式，Rust 不做协议分支。

## 远程脚本安全

远程脚本第一期支持 HTTPS 直链 JS，并允许 localhost(127.0.0.1/localhost/::1) 使用 HTTP 便于本地测试；所有远程引用都强制 `sha256`：

```txt
bp://http://127.0.0.1:PORT/parser.js?sha256=<64位hex>
```

执行流程：

1. 从规则中识别远程 URL。
2. 读取 `sha256` 参数或 fragment。
3. 下载时移除 `sha256/checksum` 查询参数。
4. 校验下载内容 sha256。
5. 原子写入 `scripts/_remote-cache/parser/<source-id>/<sha256>.js`。
6. 后续同一引用优先使用缓存。

### 远程下载约束补充

- 远程 parser 下载必须复用 `bifrost_core::direct_blocking_reqwest_client_builder()`，避免受系统代理、环境变量代理或 Bifrost 自身代理链路污染。
- 远程 parser 下载必须设置显式超时；当前实现复用 `sandbox.net.timeout_ms`，默认 5000ms。
- 超时、非 2xx 状态码、body 读取失败与 sha256 校验失败都必须返回可区分的错误信息，方便在 Traffic 的 decode script result 中直接定位为“下载阶段失败”。

非 HTTPS 且不是 localhost HTTP、没有 sha256、sha256 格式不正确、下载失败、校验失败时，解析失败并保留原始 body。

## Traffic / Search / WebUI

当前 HTTP 落库链路已经在写入 body store 前执行 `decode://`：

- decode 成功后，`request_body_ref` / `response_body_ref` 存解析后的内容。
- 同时保存 `raw_request_body_ref` / `raw_response_body_ref`。
- Search 使用 `request_body_ref` / `response_body_ref`，因此可以搜索解析后的内容。
- WebUI 详情页默认读取 `/request-body` / `/response-body`，因此展示解析后的内容。
- 需要原始内容时可以通过 raw body ref/API 查询。

## 测试计划

### 单元测试

- `bp://local-parser decode://bp` 能解析到 `bp_scripts` 与 `decode_scripts`。
- 本地 parser 脚本可通过 `ScriptType::Parser` 执行并读取 `bodyBase64`。
- 本地 parser 引用可携带 `?idl=...&psm=...` 这类参数，脚本仍能加载并可从 `ctx.scriptName` 读取完整引用。
- `build_in_bp` 可以使用 Bifrost sync token 调用 `/v4/sso/info` 换取 `bam_token`，再将其作为 BAM parse Cookie。
- 远程 parser 缺少 sha256 时失败。
- 远程 parser sha256 不匹配时失败。
- 远程 parser sha256 正确时下载、缓存、执行成功。
- 远程 parser 下载超时时失败，并返回包含 timeout 语义的错误。
- `bp://http://...` 在规则 resolver 层保持 URL 引用，不提前下载成脚本内容。
- `bp://{profile}` 在规则 resolver 层展开成脚本配置字符串。
- `decode://bp` 未配置 `bp://` 时返回可见错误且不覆盖 body。

### E2E 测试

- 本地 parser：代理请求命中 `bp://local decode://bp`，客户端仍收到原始响应，Traffic 详情响应 body 展示解析 JSON。
- 远程 parser：代理首次命中时自动下载远程 JS，Traffic 详情展示解析 JSON，缓存文件存在。
- 远程 parser 下载超时：E2E/单测至少覆盖下载阶段 timeout，确认请求不会无限等待且错误出现在 script result 中。
- Search：搜索 parser 输出中的唯一字段能命中该 Traffic 记录。
- `build_in_bp`：通过 mock `/v4/sso/info` 验证 sync token 换取 `bam_token` 后调用 mock BAM parse，Traffic 详情和 Search 均展示解析结果。
- `build_in_bp` Thrift 双向二进制：通过 BAM metadata 返回 next_agent `Healthz` refschema，发送真实 Thrift CALL frame 作为 HTTP body，上游返回真实 Thrift REPLY frame；Traffic raw body 保留二进制，默认 body 展示 `protocol=thrift-binary`、`schema_type=request/response`、`method=Healthz`、`status=ok`，`bifrost search --req-body Healthz` 与 `bifrost search --res-body ok` 均能命中。
- `build_in_bp` HTTP-RPC JSON wrapper：仅作为 Explorer OpenAPI 外层 JSON 归一化能力验证，不作为 BP 二进制协议解析验收口径。

### human_tests

新增 `human_tests/bp-protocol-parser.md`：

- `TC-BP-01` 本地 parser 解析并在 WebUI/接口详情展示。
- `TC-BP-02` 远程 parser 自动下载、校验、缓存、执行。
- `TC-BP-02A` 远程 parser 下载超时后返回明确错误，且不污染缓存。
- `TC-BP-03` Search 能搜索解析后的内容。
- `TC-BP-04` 规则层保持 `bp://` 脚本引用语义并支持 profile 风格配置。
- `TC-BP-06` 使用 next_agent PSM 验证 Bifrost Server `bam_token` 获取路径和 BAM parse 当前 pb-only 边界。
- `TC-BP-08` 使用真实 next_agent `Healthz` Thrift RPC bytes 验证 `build_in_bp` 的 BAM metadata + JS Thrift 解包路径，并验证 decoded request/response body 可搜索。
- `TC-BP-09` 使用二进制 request body + 二进制 response body 验证 WebUI raw body 与 decoded body 同时可见，不以 Explorer JSON wrapper 作为 BP 二进制验收。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 相关 E2E 脚本
- `rust-project-validate`
