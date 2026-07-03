# BP Protocol Parser 设计方案

## 背景

部分二进制协议解析工具会把请求/响应解析成文本并替换 stream body。Bifrost 默认代理语义不能直接采用这种改写行为，否则真实客户端可能收到不再兼容业务协议的 JSON。业务方希望在 Bifrost 侧“既能看懂 Kitex/Thrift/Protobuf 之类的二进制 RPC，又不影响真实上下游 bytes”。

本方案把 `bp://` 定义为二进制协议解析脚本引用，默认只配合 `decode://bp` 进入 Traffic 存储、搜索和详情展示，不改写真实上游/下游流量。Rust 层只负责识别脚本引用与远程脚本安全，不实现具体业务协议（Thrift/Protobuf/BAM/envelope）分支；具体协议解析都放在 JS parser 脚本里，配合 BAM Open API 拿元数据。

## 用户目标验证清单

### 必须实现

- 规则语法 `<pattern> bp://<script-ref> decode://bp` 能被 syntax/resolver 正确解析，`bp://` 与 `decode://bp` 成对出现才触发解析。
- `bp://<name>`：加载本地 `scripts/parser/<name>.js`。
- `bp://<name?parser-options>`：加载本地 `scripts/parser/<name>.js`，完整引用保留在 `ctx.scriptName`，脚本可自行解析 `idl` / `psm` / `service` / `method` 等参数。
- `bp://<https-url?sha256=<hex>>`：下载远程 parser 脚本，校验 sha256 后缓存到 `scripts/_remote-cache/parser/`，再执行。
- `bp://{profile}`：value profile 展开成本地脚本配置字符串；本地脚本加载时只用 `?` / `#` 前的脚本名。
- `build_in_bp` 内置 parser 从 `assets/scripts/parser/build_in_bp.js` 随版本释放到当前数据目录，升级 / 启动后覆盖为最新内容。
- Traffic 落库前执行 `decode://bp` 时，把 parser 输出写入 `request_body_ref` / `response_body_ref`；原始 Body 写入 `raw_request_body_ref` / `raw_response_body_ref`。
- Search 使用解析后的 Body 内容，可以搜到 parser 输出中的字段值。
- WebUI Traffic detail 默认展示解析后的 Body，raw body 通过 raw ref 单独查看。
- BAM token 获取遵循固定优先级（下面 “技术细节 -> BAM token” 部分），脚本不需要用户手工复制 Cookie。
- 远程 parser 强制 HTTPS，且必须带 `sha256`；本地 loopback（127.0.0.1 / localhost / ::1）允许 HTTP 以便自测。

### 必须不破坏

- 不支持 `bp://script:` / `bp://thrift:` / `bp://protobuf:` / `bp://cmd:` / `bp://wasm:` 这类 provider 前缀；Rust 层只认识 parser 脚本。
- 未配置 `decode://bp` 时，`bp://` 只作为匹配到的规则元数据，不触发解析，也不改写上下游 Body。
- 真实客户端收到的响应 Body 与真实上游发送的请求 Body 保持二进制不变。
- Bifrost 当前 SOCKS5 / HTTP CONNECT 原始 tunnel 语义不变：只落连接记录、`request_size` 和 `response_size`，不落原始 TCP payload，也不会触发 `decode://bp`。
- 远程脚本下载不受系统代理、环境变量代理或 Bifrost 自身代理链路污染。
- Body 类规则（`reqMerge` / `resMerge` 等）与 `decode://bp` 是两条独立链路，互不影响。

### 必须真实验证

- 本地 parser：真实代理请求命中后 Traffic 详情响应 Body 展示解析 JSON。
- 远程 parser：真实首次命中时自动下载远程 JS，Traffic 详情展示解析 JSON，缓存文件存在；超时 / 校验失败返回可诊断错误。
- Search：搜索 parser 输出中的唯一字段能命中该 Traffic 记录。
- `build_in_bp` 真实调用 Bifrost sync `/v4/sso/info` 换取 BAM token，再走 BAM Open API 完成 Thrift/Kitex 解包。
- 使用真实 Thrift binary frame 通过 HTTP body 走现有 body capture 链路，验证 `build_in_bp` 解包、Traffic 详情展示和 `bifrost search` 搜索解码结果。

## 产品语义

### `bp://` 是脚本引用，不是协议分支

用户侧保持 `bp://` 只表达“使用一个 parser 脚本”，业务协议输入作为脚本参数，例如：

```txt
api.example.com bp://build_in_bp?idl=file://~/work/code/nextoncall/next_agent/idl/order.thrift&service=OrderService&method=GetOrder
api.example.com decode://bp

api.example.com bp://build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
api.example.com decode://bp
```

长配置通过 value profile 收敛：

````txt
api.example.com bp://{order_bp}
api.example.com decode://bp

```order_bp
build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
```
````

Rust 层只负责保留 `bp://` 脚本引用语义：远程 URL 不在规则 resolver 中提前下载；`bp://{profile}` 可以展开成本地脚本配置字符串；本地脚本加载时只用 `?` / `#` 前的脚本名。IDL 文件读取、PSM 到 BAM 查询、协议转换与版本缓存都由 parser JS/adapter 处理。若后续确认 QuickJS 沙箱无法直接访问 BAM，则补一个通用 asset resolver helper，仍不在 Rust 内置 thrift/protobuf/BAM 分支。

### 只落库，不改写

`bp://` + `decode://bp` 是 Traffic 存储/搜索/展示专用管线，不影响客户端 ↔ 上游的真实字节。Body 类规则（`reqMerge` / `resMerge` / `reqBody` / `resBody` 等）需要在明文上执行时，用户应使用 Body 规则；如果要在“解码后的展示视图”上再做视觉级变换，也走 parser 脚本的 output，不走 Body 规则。

### 内置 `build_in_bp` 与自定义 parser

- `build_in_bp` 是推荐的 BAM 内置适配 parser，随版本发布自动覆盖数据目录下同名文件。
- 用户想在本地保留自定义 parser 时应使用其他脚本名（如 `my_bp.js`），避免升级时被覆盖。
- 远程 parser（HTTPS 直链 JS）适合分发到多设备的共享 parser，通过 `sha256` 保证内容一致。

### BAM 元数据接口

参考 BAM Open API《API 元数据 Open API》，`build_in_bp` 解析 Thrift/Kitex RPC 时使用这些只读接口：

- `/api/endpoint/list`：`psm + version/branch` 获取 endpoint 列表，按 `rpc_method` / `path` 找到 `endpoint_id`。
- `/api/endpoint/info?schema=ref`：获取目标接口 `req_type` / `resp_type`，用于构造 `Healthz_args` / `Healthz_result` 之类 RPC wrapper。
- `/api/service/refschema?raw_field=1`：保留 RPC 原字段名和 field id 的 refschema，用于按 Thrift binary field id 解包。
- `/api/service/idl`：排查和离线调试入口，用于确认 PSM 对应的原始 Thrift IDL 与版本。
- `/api/endpoint/info/example_code?custom_protocol=rpc`：生成请求/响应样例，辅助确认期望字段。

部分 parse 入口只支持 protobuf IDL，因此 `build_in_bp` 对 Thrift/Kitex 走 JS 内置 Thrift binary decoder，而不是继续依赖该类 parse 入口。

### 原始 TCP RPC 边界

Bifrost 当前 SOCKS5 / HTTP CONNECT 原始 tunnel 只落连接记录、`request_size` 和 `response_size`，不落原始 TCP payload，也不会触发 `decode://bp`。真实 Kitex TCP 流量若只是作为 tunnel 经过 Bifrost，当前不能在 Traffic 详情或 Search 中展示解码后的 RPC 字段。本期验证真实 RPC bytes 的方式是把真实 Kitex/Thrift binary frame 作为 HTTP body 进入现有 body capture 链路，验证解包、Traffic 详情展示与 `bifrost search`。后续若要支持原始 TCP tunnel 内的 RPC 解码，需要新增通用 TCP payload capture/decode 能力，且仍应把协议解析放在 parser 脚本中。

## 技术细节

### 脚本运行时

新增 `ScriptType::Parser`（`crates/bifrost-script/src/types.rs`）与目录：

```txt
scripts/
  parser/
  _remote-cache/
    parser/
```

Parser 脚本复用 decode 脚本的输出合同：

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

`ctx.scriptName` 保留完整引用（例如 `build_in_bp?psm=foo.bar.order&method=GetOrder`），parser 脚本可以自解析参数。

### BAM token 获取

`build_in_bp` 不要求用户手工复制 BAM 页面 Cookie。脚本的 token 优先级如下：

1. `bamToken`：直接传入 BAM 可用的 Cookie，例如 `ak=bifrost_v4;c_token=...`。
2. `bamTokenFile`：读取文件中的 `bam_token` / `bamToken`，或纯文本 Cookie。
3. `syncToken` / `bifrostToken`：调用 Bifrost 同步服务 `GET /v4/sso/info`，使用 `x-bifrost-token` 换取 `data.bam_token`。
4. `syncTokenFile`：读取文件中的 `token` / `syncToken` / `sync_token`，或纯文本 sync token，再执行第 3 步。
5. 默认尝试读取 `db/config.json`：若含 `bam_token` / `bamToken` 直接使用；若含 `token`，用默认同步鉴权信息接口换取 `data.bam_token`。

服务端同步信息接口负责把已登录用户信息转换为 parser 可用的 `data.bam_token`，脚本只读取该字段，不在规则中保存 Cookie。`build_in_bp` 保留 `bamAuthUrl` / `bifrostInfoUrl` / `bifrostBaseUrl` 参数，便于 E2E 或私有部署环境覆盖默认同步服务地址。

### 远程脚本安全

远程 parser 第一期支持 HTTPS 直链 JS，本地 loopback（127.0.0.1 / localhost / ::1）允许 HTTP：

```txt
bp://http://127.0.0.1:PORT/parser.js?sha256=<64位hex>
```

执行流程：

1. 从规则中识别远程 URL。
2. 读取 `sha256` 参数或 fragment。
3. 下载时移除 `sha256` / `checksum` 查询参数。
4. 校验下载内容 sha256。
5. 原子写入 `scripts/_remote-cache/parser/<source-id>/<sha256>.js`。
6. 后续同一引用优先使用缓存。

远程 parser 下载必须复用 `bifrost_core::direct_blocking_reqwest_client_builder()`，避免受系统代理、环境变量代理或 Bifrost 自身代理链路污染。必须设置显式超时；当前实现复用 `sandbox.net.timeout_ms`，默认 5000ms。超时、非 2xx 状态码、body 读取失败与 sha256 校验失败都必须返回可区分错误信息，方便在 Traffic 的 decode script result 中直接定位为“下载阶段失败”。

非 HTTPS 且不是 localhost HTTP、没有 sha256、sha256 格式不正确、下载失败、校验失败时，解析失败并保留原始 Body。

### Traffic / Search / WebUI

HTTP 落库链路在写入 body store 前执行 `decode://`：

- decode 成功：`request_body_ref` / `response_body_ref` 存解析后的内容；同时保存 `raw_request_body_ref` / `raw_response_body_ref`。
- Search 使用 `request_body_ref` / `response_body_ref`，因此可以搜索解析后的内容。
- WebUI 详情页默认读取 `/request-body` / `/response-body`，因此展示解析后的内容；raw body ref/API 单独查询原始内容。
- `decode://bp` 未配置 `bp://` 时返回可见错误且不覆盖 Body。

## CLI 交互

- `bifrost rule syntax-check`：对 `bp://` 与 `decode://bp` 成对语义、`bp://` 后缀参数、远程 URL sha256 完整性做基础校验，错误信息面向用户可读。
- `bifrost script list --type parser`：列出当前数据目录下 parser 脚本；`build_in_bp` 标记为随版本发布。
- `bifrost script show <name>`：查看 parser 内容与来源（本地 / 远程缓存 / 内置）。
- `bifrost script cache purge --parser`：清空 `scripts/_remote-cache/parser/`。
- `bifrost search --req-body <text>` / `bifrost search --res-body <text>`：命中 parser 输出中的关键字。
- `bifrost traffic get <id>`：展示解码后 Body，`--raw` 或独立子命令读取 raw body。

## Web / Admin API

- Rules 编辑器智能提示（planned）：
  - `bp://` 协议值默认给出 `build_in_bp?psm=<psm>&method=<method> decode://bp`。
  - 已存在的 parser 脚本提示为 `bp://<parser> decode://bp`，确保落库解析协议一并生成。
  - `decode://` 协议值提示包含 `bp`，用于用户分行书写时补齐 `decode://bp`。
  当前 `bifrost-core::syntax` 的 `Protocol::Bp` 占位提示仍是通用的 `bp://my-parser-script`，未生成 `build_in_bp?...` 与 `decode://bp` 的成对补全。
- Scripts 管理页面新增 `parser` tab，展示本地 / 内置 / 远程缓存三类来源。
- Traffic detail 增加 “decoded body” / “raw body” 切换（若未启用 decode 则只显示 raw body）。
- Admin API：
  - `GET /api/scripts?type=parser`：列出 parser。
  - `GET /api/scripts/parser/<name>`：读取 parser 内容与元数据。
  - `POST /api/scripts/parser`：创建 / 更新本地 parser。
  - `DELETE /api/scripts/parser/<name>`：删除本地 parser（`build_in_bp` 允许删除，下次启动会被 asset 释放覆盖回来）。
  - Traffic API `?decoded=1` / `?raw=1`：切换 body 视图。

## Sync 边界

- 本地 parser 视为用户脚本资产，允许通过现有 script sync 通道同步。`build_in_bp` 与远程 parser 缓存不参与 sync，前者由 asset 释放保证一致，后者由 sha256 URL 保证一致。
- Rules 中的 `bp://` 引用参与规则 sync；`bp://{profile}` 展开的 value profile 也参与 sync。
- BAM token 通过 sync 鉴权接口在设备本地换取，绝不出现在规则同步内容里。

## 实现切分

### Phase 1：脚本运行时 & 内置资源

- `ScriptType::Parser` 与目录结构。
- Parser 脚本执行器（复用 decode 脚本执行链路，扩展 `bodyBase64`）。
- `assets/scripts/parser/build_in_bp.js` 与启动时的 asset 释放 / 升级覆盖。
- `bifrost script list/show --type parser` CLI 与 `GET /api/scripts?type=parser`。

### Phase 2：规则识别与 decode 集成

- Syntax / resolver 支持 `bp://<name>`、`bp://<name?opts>`、`bp://{profile}`、`bp://https://…?sha256=…`。
- `decode://bp` 与 `bp://` 成对语义；未成对时的错误路径。
- Traffic 落库前的 decode 阶段调用 parser，输出写入 `request_body_ref` / `response_body_ref`。

### Phase 3：远程 parser 与安全

- 远程 URL 下载（`direct_blocking_reqwest_client_builder`、显式超时、sha256 校验、原子写入缓存）。
- 错误分类：非 HTTPS / 无 sha256 / 下载超时 / 非 2xx / body 读取失败 / sha256 不匹配。
- 单元测试 + `bifrost script cache purge --parser` CLI。

### Phase 4：BAM 集成与 human_tests

- `build_in_bp` 五级 token 优先级；`bamAuthUrl` / `bifrostInfoUrl` / `bifrostBaseUrl` 参数。
- BAM Open API 集成（`/api/endpoint/list` / `/api/endpoint/info?schema=ref` / `/api/service/refschema?raw_field=1`）。
- `bifrost search` 命中解析后字段。
- `human_tests/bp-protocol-parser.md` 与 `human_tests/readme.md`。

## 测试方案

### 单元测试

- `syntax::bp_local_parser_resolves`：`bp://local-parser decode://bp` 能解析到 `bp_scripts` 与 `decode_scripts`。
- `script::parser_type_reads_body_base64`：本地 parser 脚本可通过 `ScriptType::Parser` 执行并读取 `bodyBase64`。
- `syntax::bp_with_query_params_preserves_reference`：本地 parser 引用可携带 `?idl=...&psm=...`，脚本仍能加载并可从 `ctx.scriptName` 读取完整引用。
- `build_in_bp::bam_token_from_sync_sso_info`：使用 Bifrost sync token 调用 `/v4/sso/info` 换取 `bam_token`，再将其作为 BAM parse Cookie。
- `parser::remote_missing_sha256_fails`：远程 parser 缺少 sha256 时失败。
- `parser::remote_sha256_mismatch_fails`：远程 parser sha256 不匹配时失败。
- `parser::remote_sha256_ok_caches_and_runs`：远程 parser sha256 正确时下载、缓存、执行成功。
- `parser::remote_download_timeout`：远程 parser 下载超时时失败，并返回包含 timeout 语义的错误。
- `resolver::bp_http_url_not_prefetched`：`bp://http://...` 在规则 resolver 层保持 URL 引用，不提前下载成脚本内容。
- `resolver::bp_profile_expansion`：`bp://{profile}` 在规则 resolver 层展开成脚本配置字符串。
- `decode::bp_without_bp_ref_returns_error`：`decode://bp` 未配置 `bp://` 时返回可见错误且不覆盖 Body。

### E2E 测试

- `e2e-tests/tests/test_bp_local_parser.sh`：本地 parser，客户端仍收到原始响应，Traffic 详情响应 Body 展示解析 JSON。
- `e2e-tests/tests/test_bp_remote_parser.sh`：远程 parser 首次命中自动下载，Traffic 详情展示解析 JSON，缓存文件存在。
- `e2e-tests/tests/test_bp_remote_timeout.sh`：远程 parser 下载超时，脚本 result 中出现 timeout 错误，请求不无限等待。
- `e2e-tests/tests/test_bp_search.sh`：搜索 parser 输出中的唯一字段能命中该 Traffic 记录。
- `e2e-tests/tests/test_bp_build_in_bp.sh`：mock `/v4/sso/info` 验证 sync token 换取 `bam_token` 后调用 mock BAM parse，Traffic 详情和 Search 均展示解析结果。
- `e2e-tests/tests/test_bp_thrift_binary.sh`：`build_in_bp` Thrift 双向二进制：BAM metadata 返回 next_agent `Healthz` refschema，发送真实 Thrift CALL frame 作为 HTTP body，上游返回真实 Thrift REPLY frame；Traffic raw body 保留二进制，默认 body 展示 `protocol=thrift-binary`、`schema_type=request/response`、`method=Healthz`、`status=ok`，`bifrost search --req-body Healthz` 与 `bifrost search --res-body ok` 均能命中。
- `build_in_bp` HTTP-RPC JSON wrapper：仅作为 Explorer OpenAPI 外层 JSON 归一化能力验证，不作为 BP 二进制协议解析验收口径。

### 真实场景测试 human_tests

新增 `human_tests/bp-protocol-parser.md`：

- `TC-BP-01`：本地 parser 解析并在 WebUI / 接口详情展示。
- `TC-BP-02`：远程 parser 自动下载、校验、缓存、执行。
- `TC-BP-02A`：远程 parser 下载超时后返回明确错误，且不污染缓存。
- `TC-BP-03`：Search 能搜索解析后的内容。
- `TC-BP-04`：规则层保持 `bp://` 脚本引用语义并支持 profile 风格配置。
- `TC-BP-06`：使用 next_agent PSM 验证 Bifrost Server `bam_token` 获取路径和 BAM parse 当前 pb-only 边界。
- `TC-BP-08`：使用真实 next_agent `Healthz` Thrift RPC bytes 验证 `build_in_bp` 的 BAM metadata + JS Thrift 解包路径，并验证 decoded request/response body 可搜索。
- `TC-BP-09`：使用二进制 request body + 二进制 response body 验证 WebUI raw body 与 decoded body 同时可见，不以 Explorer JSON wrapper 作为 BP 二进制验收。

human_tests 服务启动统一使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 和 `--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-script parser`
- `cargo test -p bifrost-core rule::resolver bp`
- `cargo test -p bifrost-admin scripts`
- 相关 E2E 脚本（`test_bp_local_parser.sh`、`test_bp_remote_parser.sh`、`test_bp_remote_timeout.sh`、`test_bp_search.sh`、`test_bp_build_in_bp.sh`、`test_bp_thrift_binary.sh`）
- `cargo test --workspace --all-features`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：`bp://` + `decode://bp` 成对触发、真实字节不被改写、Traffic decoded body 展示、Search 命中、远程脚本安全、BAM token 优先级。
- 复核 diff：`bifrost-script` / `bifrost-core::syntax` / `bifrost-core::rule::resolver` / `bifrost-admin::scripts` / `assets/scripts/parser/build_in_bp.js` / e2e / human_tests。
- 重点 review：`bp://https://...` 是否被误在 resolver 提前下载；`decode://bp` 缺失 `bp://` 时是否覆盖 body；远程下载是否走 direct client。
- 复测：focused 单元测试、E2E 全量、human_tests 手动执行。

### 第 2 轮

- 复核第 1 轮发现问题的修复；再次检查 `git status --short`、`git diff`、新增文件与 human_tests 索引。
- 重点 review：内置 `build_in_bp` 升级覆盖是否会误删用户脚本；远程缓存目录结构在多引用 sha256 情况下是否互不干扰；`bp://{profile}` 展开后加载脚本名的边界（`?` / `#` / 空参数）。
- 复测：失败路径重跑、`cargo test --workspace --all-features`、`rust-project-validate`。

## 风险与决策点

- Rust 层不引入协议分支：Thrift / Protobuf / BAM / Kitex 解包全部放在 JS parser，避免 Rust 侧维护多种协议实现，也让业务方可以自带 parser。
- 远程脚本沙箱：JS parser 运行在 QuickJS 沙箱，不允许直接读文件；若确认需要访问 IDL 或 BAM，通过 `ctx` 提供的 asset resolver helper，仍不在 Rust 内置分支。
- Sync 边界：`build_in_bp` 与远程缓存不参与 script sync，避免多设备版本漂移；本地自定义 parser 走 script sync。
- BAM token 存储：只在运行时从 sync 服务换取，不落规则；防止规则分享时泄露 Cookie。
- 原始 TCP tunnel RPC 解码：本期不支持，后续通过通用 TCP payload capture/decode 新能力实现。
- 内置 parser 命名冲突：若用户自定义脚本使用 `build_in_bp` 名字，升级时会被覆盖；文档需明确提示，CLI `bifrost script list --type parser` 应对内置来源做标注。
