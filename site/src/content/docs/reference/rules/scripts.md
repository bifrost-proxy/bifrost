---
title: "脚本规则"
description: "在 Rules 中接入脚本能力的方式。"
editUrl: false
sidebar:
  label: "脚本规则"
  order: 300
---

> 此页面由 `docs/rules/scripts.md` 自动同步生成。
# 脚本规则

本章介绍通过 JavaScript 脚本对请求/响应进行处理的能力：

- `reqScript://{script_name}`：请求阶段脚本（转发到上游前执行）
- `resScript://{script_name}`：响应阶段脚本（收到上游响应后执行）
- `decode://{script_name}`：body decode 脚本（请求/响应落库前执行，用于解码/脱敏/格式化）
- `bp://{parser_script}` + `decode://bp`：二进制协议 parser 脚本（请求/响应落库前解析，用于 Traffic 详情与搜索）

> 说明：脚本名称对应 `~/.bifrost/scripts/{type}/{script_name}.js`。

---

## reqScript

### 语法

```
pattern reqScript://my-script
```

### 可用全局变量

| 变量 | 说明 |
| --- | --- |
| `request` | 请求对象（可修改 `method` / `headers` / `body`） |
| `ctx` | 执行上下文（含 `requestId` / `values` / `matchedRules` 等） |
| `log` / `console` | 日志（会在管理端脚本测试面板展示） |
| `file` | 文件 API（受沙箱目录与白名单限制） |
| `net` | 网络 API（可开关/限速/限超时） |

### 示例

```javascript
// 给所有请求加 header，并记录到沙箱文件
request.headers["X-Debug-Id"] = ctx.requestId;
file.appendText("state/trace.log", ctx.requestId + "\n");
```

---

## resScript

### 语法

```
pattern resScript://my-script
```

### 可用全局变量

| 变量 | 说明 |
| --- | --- |
| `response` | 响应对象（可修改 `status` / `statusText` / `headers` / `body`） |
| `ctx` | 执行上下文 |
| `log` / `console` | 日志 |
| `file` | 文件 API |
| `net` | 网络 API |

### 示例

```javascript
// 给响应加调试头
response.headers["X-Processed-By"] = "bifrost";
```

---

## decode

decode 脚本用于在 **落库之前** 对请求/响应的 body 做解码、脱敏、压缩/解压后的二次处理等。

### 语法

```
pattern decode://my-decode
```

### 执行阶段

- `ctx.phase === "request"`：解码请求体（此时 `response === null`）
- `ctx.phase === "response"`：解码响应体（此时 `response.request` 带有请求快照）
- `ctx.phase === "websocket_send"`：解码 WebSocket 客户端→服务端帧 payload（payload 作为 requestBodyBytes）
- `ctx.phase === "websocket_recv"`：解码 WebSocket 服务端→客户端帧 payload（payload 作为 responseBodyBytes）

### 内置解码器

- `decode://utf8`：内置 UTF-8（lossy）解码器
- `decode://default`：等价于 `decode://utf8`

### 输出约定

decode 脚本需要输出一个 JSON 对象：

```javascript
// 推荐：直接 return
return { code: "0", data: "decoded text", msg: "" };

// 也支持：设置 ctx.output
// ctx.output = { code: "0", data: "decoded text", msg: "" };
```

- `code === "0"`：成功，`data` 会作为新的 body 内容用于落库
- 否则：`msg` 会作为新的 body 内容用于落库（便于排查失败原因）

---

## bp

`bp` 用于把二进制协议 parser 绑定到当前规则。它本身不直接改写请求/响应，必须配合 `decode://bp` 才会在 Traffic 落库前执行 parser。解析后的内容会写入默认 body，因此 WebUI Traffic 详情与 `bifrost search --req-body/--res-body` 搜索到的是解析后的文本；原始 body 会保留在 raw body 中用于回溯。

### 语法

```txt
pattern bp://local-parser decode://bp
pattern bp://local-parser?option=value decode://bp
pattern bp://https://example.com/parser.js?sha256=<64位hex> decode://bp
```

### 本地 parser

本地 parser 放在 `scripts/parser/` 目录中：

```txt
~/.bifrost/scripts/parser/build_in_bp.js
```

`build_in_bp` 是推荐的 BP adapter 脚本名。Bifrost 启动时会把仓库内置的 `assets/scripts/parser/build_in_bp.js` 自动释放到当前数据目录的 `scripts/parser/build_in_bp.js`，并在后续升级/启动时覆盖为随版本发布的最新内容。用户开箱即可通过 `bp://build_in_bp ... decode://bp` 使用；如果需要完全自定义实现，建议另建 parser 脚本名，避免被内置脚本覆盖。

规则示例：

```bash
api.example.com bp://build_in_bp decode://bp
```

规则编辑器会对 `bp://` 给出成对提示，优先生成 `bp://build_in_bp?... decode://bp`，也会在 `decode://` 后提示 `bp`，避免只绑定 parser 却忘记启用落库前解析。

parser 脚本复用 decode 输出约定：

```javascript
ctx.output = {
  code: "0",
  data: JSON.stringify({
    phase: ctx.phase,
    bodyBase64: response.bodyBase64 || request.bodyBase64,
  }),
  msg: "",
};
```

### 传入 IDL 文件

如果 parser 需要 IDL 文件，推荐把 IDL 作为脚本参数传入。Bifrost 只负责加载 `?` 前面的 parser 脚本，完整引用会保留在 `ctx.scriptName`，由脚本自己解析参数。

```bash
api.example.com bp://build_in_bp?idl=file:///Users/eden/work/code/nextoncall/next_agent/idl/order.thrift&service=OrderService&method=GetOrder decode://bp
```

适合场景：

- 本地已有 thrift/protobuf IDL 文件。
- 希望规则中明确写出 `service` / `method`。
- parser 脚本自己管理 IDL 读取、编译和缓存。

### 传入 PSM 并从 BAM 查找/转换

如果团队习惯通过 PSM 找协议定义，可以把 PSM 作为 parser 参数：

```bash
api.example.com bp://build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam decode://bp
```

推荐约定：

| 参数 | 说明 |
| --- | --- |
| `psm` | 服务 PSM，例如 `foo.bar.order` |
| `idlSource` | IDL 来源，BAM 场景使用 `bam` |
| `service` | 协议中的 service 名称 |
| `method` | 协议中的 method 名称 |
| `protocol` / `format` | 可选，`thrift` / `thrift-binary` / `kitex` 时使用 JS 内置 Thrift binary decoder；`http-rpc` 仅用于 BAM/Explorer RPC 测试 OpenAPI 外层 JSON 归一化，不代表 BP 二进制解析 |
| `schemaType` / `schema_type` | 可选，`request` / `response`；不传时按当前 decode 阶段判断 |
| `endpointId` / `endpoint_id` | 可选，直接指定 BAM endpoint，避免脚本先调用 endpoint list 过滤 |
| `idl` | 本地 IDL 文件路径，使用 `file://` URL |
| `version` | 可选，指定 BAM/IDL 版本 |
| `bamToken` | 可选，直接传入 BAM Cookie；优先级最高，不建议写入共享规则 |
| `bamTokenFile` | 可选，从文件读取 `bam_token` / `bamToken` 或纯文本 BAM Cookie |
| `syncToken` / `bifrostToken` | 可选，Bifrost 同步服务 token；脚本会调用 `/v4/sso/info` 换取 `data.bam_token` |
| `syncTokenFile` | 可选，从文件读取 `token` / `syncToken` / `sync_token` 或纯文本 sync token |
| `bamAuthUrl` / `bifrostInfoUrl` / `bifrostBaseUrl` | 可选，覆盖默认同步鉴权信息接口 |
| `bamBaseUrl` | 可选，覆盖 BAM Open API 默认地址，测试/私有环境可使用 |

Bifrost 不在 Rust 层内置 thrift/protobuf/BAM 逻辑。PSM 查询、BAM API 调用、IDL 下载、协议转换和版本缓存都应该由 parser 脚本或 parser adapter 完成。这样不同系统的 BP 协议定义不同，也不需要改 Bifrost 代理核心。

`build_in_bp` 默认会尝试读取 `db/config.json`：如果文件里有 `bam_token` / `bamToken` 就直接使用；如果有 Bifrost 同步登录的 `token`，会请求默认同步鉴权信息接口，通过 `x-bifrost-token` 换取 `data.bam_token`，再调用 BAM parse。共享规则里推荐只写 `psm` / `service` / `method`，凭证放在本机配置或私有文件中。

Thrift/Kitex RPC 场景推荐写法：

```bash
api.example.com bp://build_in_bp?protocol=thrift&psm=flow.devops.next_agent&version=1.0.77&method=Healthz decode://bp
```

`build_in_bp` 会通过 BAM Open API 的 `/api/endpoint/list`、`/api/endpoint/info?schema=ref`、`/api/service/refschema?raw_field=1` 获取元数据，再在脚本内按 Thrift binary field id 解包。当前 BAM `binary_tools/parse` 对部分 Thrift IDL 会返回 `only support pb idl`，这种场景应使用 `protocol=thrift` 路径。

通过 RPC 测试 OpenAPI 发起调用时可使用外层 JSON 归一化：

```bash
api.example.com bp://build_in_bp?protocol=http-rpc&psm=foo.bar.order&version=1.0.0&method=Healthz decode://bp
```

调用方请求 RPC 测试 OpenAPI，并按平台要求提供鉴权信息。这个接口的 HTTP body 本身是 JSON wrapper，`build_in_bp` 会把 request/response 归一化为 `protocol=http-rpc`、目标 `psm`、`method`、`endpoint_path` 和 `data`，因此 WebUI 详情与 `bifrost search --req-body/--res-body` 搜索的是解析后的内容。注意这不是 BP 二进制解析验收；验证 BP 二进制时必须让 Thrift/Kitex binary frame 直接进入 HTTP/WebSocket body capture 链路，并检查 raw body 仍为二进制、decoded body 为 parser 输出。

注意：Bifrost 当前不会捕获 SOCKS5 / CONNECT 原始 TCP tunnel 的 payload；真实 Kitex TCP tunnel 只会记录连接和大小，无法触发 `decode://bp`。要在 Traffic/Search 中查看 decoded RPC 字段，需要该二进制帧进入 HTTP/WebSocket body capture 链路，或者后续新增通用 TCP payload capture。

### 使用 Values 收敛长配置

长参数建议放到 Values / 内嵌值里，让规则可读性更好：

````bash
api.example.com bp://{order_bp} decode://bp

```order_bp
build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
```
````

展开后等价于：

```bash
api.example.com bp://build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam decode://bp
```

### 远程 parser 脚本

远程 parser 适合团队统一维护 adapter。远程脚本必须带 `sha256`，下载后会校验并缓存到 `scripts/_remote-cache/parser/`：

```bash
api.example.com bp://https://example.com/bifrost/build_in_bp.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp
```

本地调试时允许 localhost HTTP：

```bash
api.example.com bp://http://127.0.0.1:8080/build_in_bp.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp
```

约束：

- 线上远程脚本必须使用 HTTPS。
- HTTP 仅允许 `127.0.0.1` / `localhost` / `::1`。
- `sha256` 必须是 64 位十六进制字符串。
- 下载失败、校验失败或脚本执行失败时，不覆盖 body，错误会展示在 Traffic 详情的 parser 执行结果中。

### 搜索解析后的内容

`decode://bp` 成功后，解析结果会写入 `request_body_ref` / `response_body_ref`，因此 CLI 搜索默认搜索到解析后的内容：

```bash
# 搜索解析后的响应体
bifrost search "order_id" --res-body

# 输出 JSON，便于脚本断言
bifrost search "order_id" --res-body --format json
```

如果需要对比原始二进制内容，可以在 Traffic 详情中查看 raw body，或通过 raw body API 获取原始内容。

### 常见配置模板

#### 本地 IDL

```bash
api.example.com host://127.0.0.1:8080
api.example.com bp://build_in_bp?idl=file:///Users/eden/work/code/nextoncall/next_agent/idl/order.thrift&service=OrderService&method=GetOrder
api.example.com decode://bp
```

#### PSM + BAM

````bash
api.example.com host://127.0.0.1:8080
api.example.com bp://{order_bp}
api.example.com decode://bp

```order_bp
build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder&idlSource=bam
```
````

#### 团队远程 adapter

```bash
api.example.com bp://https://example.com/parser/build_in_bp.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp
```

---

## 沙箱与配置

### file API

- 读写路径默认相对 `sandbox.file.sandbox_dir`（通常为 `scripts/_sandbox/`）
- 相对路径禁止 `..`，避免目录穿越
- 绝对路径仅允许访问 `sandbox.file.allowed_dirs` 白名单中的目录
- 单次读写大小受 `sandbox.file.max_bytes` 限制

可用方法：

- `file.readText(path)`
- `file.writeText(path, content)`
- `file.appendText(path, content)`
- `file.exists(path)`
- `file.remove(path)`
- `file.listDir(path?)`

### net API

- `net.fetch(url, optionsJson?)` / `net.request(...)` 返回 JSON 字符串，建议 `JSON.parse(...)`
- 仅允许 `http/https`
- 请求/响应体大小与超时分别受 `sandbox.net.max_request_bytes` / `sandbox.net.max_response_bytes` / `sandbox.net.timeout_ms` 限制

`optionsJson` 示例：

```javascript
var resp = JSON.parse(net.fetch("https://httpbin.org/get", JSON.stringify({
  method: "GET",
  timeoutMs: 3000,
  headers: { "X-Debug": "1" },
})));
log.info("status:", resp.status);
```

### config.toml

配置位于 `~/.bifrost/config.toml` 的 `sandbox` 字段下：

```toml
[sandbox.file]
sandbox_dir = "_sandbox"              # 相对 scripts/ 的目录名，或绝对路径
allowed_dirs = ["/var/log"]           # 允许访问的系统目录（绝对路径）
max_bytes = 1048576                    # 单次文件读写最大字节数

[sandbox.net]
enabled = true
timeout_ms = 5000
max_request_bytes = 262144
max_response_bytes = 1048576

[sandbox.limits]
timeout_ms = 10000
max_memory_bytes = 33554432
max_decode_input_bytes = 2097152
max_decompress_output_bytes = 10485760
```

说明：

- `max_memory_bytes`：QuickJS 沙箱内存上限，超出会导致脚本失败
- `max_decode_input_bytes`：decode 输入 bytes 上限，超过会跳过 decode（避免大 payload 解码造成性能/内存风险）
- `max_decompress_output_bytes`：HTTP body 解压输出上限，超过会放弃解压并回退到原始压缩数据（避免压缩炸弹）

### 管理端动态修改

在管理端 **Scripts** 页面左侧目录树顶部点击齿轮按钮，可以在线修改 `sandbox` 配置，并持久化到 `config.toml`。
