# 脚本规则

本章介绍通过 JavaScript 脚本对请求/响应进行处理的能力：

- `reqScript://{script_name}`：请求阶段脚本（转发到上游前执行）
- `resScript://{script_name}`：响应阶段脚本（收到上游响应后执行）
- `resStreamScript://{script_name}`：SSE 响应流脚本（逐事件处理并立即下发）
- `decode://{script_name}`：body decode 脚本（请求/响应落库前执行，用于解码/脱敏/格式化）
- `bp://{parser_script}` + `decode://bp`：二进制协议 parser 脚本（请求/响应落库前解析，用于 Traffic 详情与搜索）

> 推荐：通过 CLI 或 AI 生成、保存、分享规则时，优先使用同一规则文件内的 inline block。这样规则和脚本不会分离，复制一份规则即可复现完整行为。Scripts 管理页更适合交互式试跑、人工编辑和复用本机脚本。

命名脚本仍然受支持。脚本名称对应 `~/.bifrost/scripts/<dir>/<script_name>.js`，协议前缀与目录的映射为：reqScript → `scripts/request/`，resScript / resStreamScript → `scripts/response/`，decode → `scripts/decode/`，bp(parser) → `scripts/parser/`。例如 `reqScript://audit_req` 对应 `~/.bifrost/scripts/request/audit_req.js`。

## 推荐的 inline block 写法

`reqScript`、`resScript` 和 `resStreamScript` 都可以引用当前规则文件中的代码块：

````text
api.example.com reqScript://{add_trace} resStreamScript://{map_sse}

```add_trace
request.headers["X-Trace-Id"] = ctx.requestId;
```

```map_sse
stream.mode = "transform";
stream.onEvent = (event) => ({ event: event.event, data: event.data });
```
````

编辑器会识别这些 block 变量，提供协议补全、变量跳转、hover 说明和语法检查。已定义的 inline 脚本不会被误报为“命名脚本文件不存在”。

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

## resStreamScript：真 SSE 流式脚本

`resStreamScript` 为每条 HTTP 响应流创建一个独立、持久的 QuickJS 上下文。闭包和顶层变量会在同一条流的多个事件之间保留；不同请求之间完全隔离。

stream worker 的生命周期与该 HTTP 响应一致，不受普通脚本单次执行超时影响。沙箱的 `timeout_ms` 会在每一次 `stream.next()`、`stream.onEvent()`、`stream.onEnd()` 调用开始时重新计时，只限制单次 JavaScript 回调的 CPU 执行时间；等待上游下一条 SSE event 的 10 分钟或 20 分钟不会累计到这个超时中。

它有两种模式，两种都不会先收集完整响应再回放。

### Transform：逐事件变换上游 SSE

```javascript
stream.mode = "transform";
let index = 0;

stream.onEvent = (event) => {
  index += 1;
  return {
    event: "mapped.delta",
    data: JSON.stringify({ index, upstream: event.data }),
  };
};

stream.onEnd = () => "data: [DONE]\n\n";
```

Bifrost 只缓存尚未组成完整 SSE event 的少量字节。一旦读到 `\n\n` 或 `\r\n\r\n`，立即调用一次 `stream.onEvent(event)`，并在回调返回后立即向客户端发送输出；不会等待上游 EOF 或 `[DONE]`。

传入的 `event` 字段为：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string / null | 上游 SSE `id:` |
| `event` | string / null | 上游 SSE `event:` |
| `data` | string | 合并后的上游 SSE `data:` |
| `retry` | number / null | 上游 SSE `retry:` |

### Mock：脚本主动逐步生成 SSE

```javascript
stream.mode = "mock";
let index = 0;

stream.next = () => {
  index += 1;
  return {
    output: { event: "mock.delta", data: String(index) },
    delayMs: 100,
    done: index === 3,
  };
};
```

Bifrost 每次只调用一次 `stream.next()`，把本次返回的输出发给客户端后才进入下一步。`delayMs` 是本次输出发出后的等待时间；它不会触发预生成或整段缓存。

### 回调返回值

`stream.next()`、`stream.onEvent(event)`、`stream.onEnd()` 可以返回：

- 原始字符串，例如 `"data: hello\\n\\n"`。
- SSE event 对象：`{ id?, event?, data, retry? }`。
- 上述值的数组，数组元素按顺序立即下发。
- step 对象：`{ output }` 或 `{ outputs: [...] }`，并可带 `delayMs`、`done`。
- `null` / `undefined`，表示本次不输出。

输出经过容量为 8 的有界队列传播下游背压。客户端断开后，接收端关闭，流任务停止读取上游并释放该请求的脚本 worker。网络 body frame 可以任意拆分，不会被脚本层截断；Bifrost 会拼到完整 SSE event 后再调用脚本。单个完整 event 的安全上限为 16 MiB，超限会返回明确的 SSE error 并终止该流，绝不会静默截断或丢弃部分数据。

### 响应头与组合限制

- Transform 模式要求上游响应为 `Content-Type: text/event-stream`。
- Mock 模式可配合 `resHeaders://Content-Type=text/event-stream` 把普通上游响应替换为脚本生成的流。
- 当前一条匹配结果只允许一个 `resStreamScript`，避免多个有状态流转换器的顺序语义不清。
- `resScript` 用于完整 body 修改，会收集响应；实时 SSE 处理应使用 `resStreamScript`。

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
- `ctx.phase === "websocket_send"`：解码 WebSocket 客户端→服务端帧 payload（payload 通过 `request` 对象暴露，此时 `response === null`）
- `ctx.phase === "websocket_recv"`：解码 WebSocket 服务端→客户端帧 payload（沙箱仅在 `response` 阶段填充 `response` 对象；WebSocket 阶段一律走 `request` 对象路径，因此 `response === null`，帧 payload 仍通过 `request` 暴露——读取 payload 请用 `request.bodyBase64` / `request.body`，不要依赖 `response.bodyBase64`）

### 内置解码器

- `decode://utf8`：内置 UTF-8（lossy）解码器
- `decode://default`：等价于 `decode://utf8`

### 输出约定

decode 脚本需要输出一个 JSON 对象：

```javascript
// 设置 ctx.output 输出结果
ctx.output = { code: "0", data: "decoded text", msg: "" };
```

> 注意：decode 脚本体在 QuickJS 中按顶层脚本（而非函数体）求值，因此**不能**用顶层 `return` 输出——`return { ... }` 会触发 `Script execution failed: Exception generated by QuickJS: return not in a function` 并导致 decode 失败、body 不被覆盖。请统一通过 `ctx.output = { ... }` 输出。

- `code === "0"`：成功，`data` 会作为新的 body 内容用于落库
- 否则（`code != "0"`）：HTTP 请求/响应 decode 不覆盖 body（保留原内容并终止 decode 链），`msg` 仅作为错误信息记录在 Traffic 详情的执行结果中；仅 WebSocket decode 阶段会把 `msg` 作为新的帧 payload 落库。

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

> 注意：以上示例适用于 HTTP 请求/响应阶段。WebSocket decode（`websocket_send` / `websocket_recv`）阶段 `response === null`，帧 payload 只通过 `request` 对象暴露，因此应直接读取 `request.bodyBase64` / `request.body`，不要依赖 `response.bodyBase64`。

### 传入 IDL 文件

如果 parser 需要 IDL 文件，推荐把 IDL 作为脚本参数传入。Bifrost 只负责加载 `?` 前面的 parser 脚本，完整引用会保留在 `ctx.scriptName`，由脚本自己解析参数。

```bash
api.example.com bp://build_in_bp?idl=file:///path/to/project/idl/order.thrift&service=OrderService&method=GetOrder decode://bp
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
api.example.com bp://build_in_bp?idl=file:///path/to/project/idl/order.thrift&service=OrderService&method=GetOrder
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
