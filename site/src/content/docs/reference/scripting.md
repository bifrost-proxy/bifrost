---
title: "脚本能力"
description: "QuickJS 脚本能力、限制与使用方式。"
editUrl: false
sidebar:
  label: "脚本能力"
  order: 150
---

> 此页面由 `docs/scripts.md` 自动同步生成。
# Scripts 管理与脚本开发指南

本篇文档介绍 Bifrost 管理端 **Scripts** 模块的使用方式（创建/保存/测试/组织脚本），并给出常见应用场景与示例代码。

> 规则侧如何引用脚本（`reqScript://` / `resScript://` / `decode://` / `bp://`）请参考：`docs/rules/scripts.md`。

---

## 1. 脚本类型与执行时机

Bifrost 当前实现支持四类脚本：

1. **Request Script**：转发到上游前执行，可修改请求（方法/头/body）。
2. **Response Script**：收到上游响应后执行，可修改响应（状态码/头/body）。
3. **Decode Script**：用于对 body 做“解码/脱敏/格式化”等处理（常用于展示与落库前的处理）。
4. **Parser Script**：用于 `bp://...` + `decode://bp` 的二进制协议解析。它不会改写真实客户端/上游流量，只影响 Traffic 落库、详情展示与搜索。

管理端 Scripts 页面会展示 request / response / decode / parser 四类脚本，列表、保存、删除、重命名、测试走 Admin API `/_bifrost/api/scripts/*`。

---

## 2. 在管理端创建/保存/测试

### 2.1 新建

1. 进入管理端 **Scripts** 页面。
2. 点击 `Req` / `Res` / `Dec` 创建不同类型脚本。
3. 编辑器会自动填入默认模板（模板来源：`web/src/stores/useScriptsStore.ts:5`）。

### 2.2 命名与目录

脚本名会映射到磁盘路径（允许用 `/` 做目录分层）。

- 存储根目录：`{data_dir}/scripts`
  - 默认 `data_dir=~/.bifrost`（可用 `BIFROST_DATA_DIR` 覆盖：`crates/bifrost-storage/src/data_dir.rs:10`）
- 单个脚本文件路径：`{data_dir}/scripts/{type}/{name}.js`
  - `type ∈ {request,response,decode,parser}`（目录由 `crates/bifrost-script/src/engine.rs` 创建）
  - 远程 parser 脚本缓存位于 `{data_dir}/scripts/_remote-cache/parser/`
  - 脚本名允许包含 `/`，会对应子目录（`crates/bifrost-script/src/engine.rs:156`）

脚本名限制（保存时会校验）：

- 不能为空，长度 ≤ 128
- 不能以 `/` 开头或结尾，不能包含 `..`，不能包含 `//`
- 仅允许：字母数字、`-`、`_`、`/`

对应实现：`crates/bifrost-script/src/engine.rs:936`。

### 2.3 保存

保存后脚本会落盘到 `scripts/{type}/{name}.js`，并在左侧树中出现。

### 2.4 测试

Scripts 页面提供测试能力，会把执行日志（`log/info/warn/error`）与修改结果展示出来。

说明：脚本运行环境为 **QuickJS**，同步执行，不支持 `async/await`（见 `crates/bifrost-script/src/sandbox.rs:99`）。

---

## 3. 运行时可用对象（与限制）

### 3.1 通用对象

- `ctx`：上下文（`requestId/scriptName/scriptType/values/matchedRules/phase`），注入逻辑见 `crates/bifrost-script/src/sandbox.rs:623`。
- `log` / `console`：日志对象（`log.debug/info/warn/error`），注入逻辑见 `crates/bifrost-script/src/sandbox.rs:522`。
- `file`：文件 API（受沙箱目录与白名单限制），注入逻辑见 `crates/bifrost-script/src/sandbox.rs:1092`。
- `net`：网络 API（可开关/限超时/限包体大小），注入逻辑见 `crates/bifrost-script/src/sandbox.rs:1261`。

注意：建议始终先判断 `file.enabled` / `net.enabled`，避免在被禁用时脚本报错。

### 3.2 Request Script

- 全局对象：`request`
- 可修改字段：`request.method` / `request.headers` / `request.body`
- 其他字段为快照（改了不会生效）：`url/host/path/protocol/clientIp/clientApp`

请求对象注入：`crates/bifrost-script/src/sandbox.rs:687`。

### 3.3 Response Script

- 全局对象：`response`
- 可修改字段：`response.status` / `response.statusText` / `response.headers` / `response.body`
- `response.request` 为原始请求快照（修改无效）

响应对象注入：`crates/bifrost-script/src/sandbox.rs:739`。

### 3.4 Decode Script

- `ctx.phase` 表示当前阶段（通常为 `"request"` 或 `"response"`）
- 当 `ctx.phase === "request"` 时：`response === null`
- 可用字段（用于二进制/大包预览）：
  - `request.bodyHex` / `request.bodySize` / `request.bodyHexTruncated` / `request.bodyTextTruncated`
  - `response.bodyHex` / `response.bodySize` / `response.bodyHexTruncated` / `response.bodyTextTruncated`

decode 注入与截断逻辑：`crates/bifrost-script/src/sandbox.rs:811`。

decode 输出约定：脚本需要输出 `{ code, data, msg }`（支持 `return` / `ctx.output` / 全局 `output`），解析逻辑：`crates/bifrost-script/src/sandbox.rs:1011`。

### 3.5 Parser Script

Parser Script 是 `bp://<parser_ref>` 的执行目标，常用于 thrift/protobuf/私有二进制协议在 Traffic 详情中的可读化展示。

- 存储目录：`{data_dir}/scripts/parser/{name}.js`
- 内置脚本：Bifrost 启动时会自动释放并覆盖 `{data_dir}/scripts/parser/build_in_bp.js`
- 规则引用：`api.example.com bp://build_in_bp decode://bp`
- 可带参数：`bp://build_in_bp?psm=foo.bar.order&service=OrderService&method=GetOrder`
- 远程脚本：`bp://https://example.com/parser.js?sha256=<64位hex>`
- 本地调试允许 `http://127.0.0.1` / `localhost` / `::1`，其他远程 HTTP 会被拒绝；线上远程脚本必须使用 HTTPS 并提供 `sha256`

Parser 复用 decode 输出约定，成功时返回 `{ code: "0", data, msg: "" }`。`data` 会替换落库后的 body 文本用于详情展示与 `bifrost search --req-body/--res-body`，但不会改写真实转发流量。

示例：

```javascript
var bytes = response && response.bodyBase64 ? response.bodyBase64 : request.bodyBase64;
return {
  code: "0",
  data: JSON.stringify({ phase: ctx.phase, bodyBase64: bytes }),
  msg: "",
};
```

---

## 4. 常见应用场景与示例代码

> 说明：下面示例均可直接粘贴到 Scripts 编辑器中。Header 读取建议做大小写无关处理。

### 4.1 Request：注入鉴权 / 追踪信息

```javascript
// 统一注入追踪头
request.headers["X-Request-ID"] = ctx.requestId;

// 从 Values 中读取 token（在 Values 页面维护）
var apiToken = ctx.values["API_TOKEN"];
if (apiToken) {
  request.headers["Authorization"] = "Bearer " + apiToken;
}

log.info("request prepared", request.method, request.url);
```

### 4.2 Request：按条件改写 JSON body（兼容 header 大小写）

```javascript
function getHeader(headers, name) {
  var target = String(name || "").toLowerCase();
  for (var k in headers) {
    if (String(k).toLowerCase() === target) return headers[k];
  }
  return "";
}

var ct = getHeader(request.headers, "content-type");
if (request.body && String(ct).toLowerCase().includes("application/json")) {
  try {
    var obj = JSON.parse(request.body);
    obj._debug = { requestId: ctx.requestId, script: ctx.scriptName };
    request.body = JSON.stringify(obj);
  } catch (e) {
    log.error("json parse failed:", e.message);
  }
}
```

### 4.3 Response：给响应加调试信息 / 统一 CORS

```javascript
response.headers["X-Processed-By"] = "bifrost";
response.headers["X-Request-ID"] = ctx.requestId;

// CORS（示例：按需调整）
response.headers["Access-Control-Allow-Origin"] = "*";
response.headers["Access-Control-Allow-Headers"] = "*";
response.headers["Access-Control-Allow-Methods"] = "*";
```

### 4.4 Response：脱敏 JSON 响应

```javascript
function getHeader(headers, name) {
  var target = String(name || "").toLowerCase();
  for (var k in headers) {
    if (String(k).toLowerCase() === target) return headers[k];
  }
  return "";
}

var ct = getHeader(response.headers, "content-type");
if (response.body && String(ct).toLowerCase().includes("application/json")) {
  try {
    var data = JSON.parse(response.body);
    if (data && data.token) data.token = "***";
    if (data && data.password) data.password = "***";
    response.body = JSON.stringify(data);
  } catch (e) {
    log.error("json parse failed:", e.message);
  }
}
```

### 4.5 Decode：输出可读文本预览（处理 response 为 null）

```javascript
log.info("decode phase:", ctx.phase);

var text = "";
if (ctx.phase === "request") {
  text = request.body || "";
  if (request.bodyTextTruncated) {
    log.warn("request.body is truncated; consider using request.bodyHex");
  }
} else {
  text = (response && response.body) ? response.body : "";
  if (response && response.bodyTextTruncated) {
    log.warn("response.body is truncated; consider using response.bodyHex");
  }
}

return { code: "0", data: text, msg: "" };
```

### 4.6 通用：使用 file/net（务必判断 enabled）

```javascript
if (file.enabled) {
  file.appendText("state/trace.log", ctx.requestId + "\n");
}

if (net.enabled) {
  var resp = JSON.parse(net.fetch("https://httpbin.org/get"));
  log.info("net status:", resp.status);
}
```

---

## 5. 沙箱配置与安全建议

### 5.1 重要配置项

脚本沙箱配置通过 Admin API `/_bifrost/api/config/sandbox` 读取/更新：

- 读取：`crates/bifrost-admin/src/handlers/config.rs:143`
- 更新并持久化：`crates/bifrost-admin/src/handlers/config.rs:186`

配置结构定义：`crates/bifrost-storage/src/unified_config.rs:42`。

常用字段：

- `sandbox.file.sandbox_dir`：脚本沙箱工作目录（默认 `_sandbox`；相对 `scripts/` 或绝对路径）
- `sandbox.file.allowed_dirs`：允许访问的系统目录白名单（必须绝对路径）
- `sandbox.net.enabled`：是否允许 `net.fetch`
- `sandbox.limits.timeout_ms` / `sandbox.limits.max_memory_bytes`：执行超时与内存限制

默认值（未修改 `config.toml` 时）：

- `sandbox.limits.timeout_ms = 10000`（10s），超时会中断脚本执行并返回错误
- `sandbox.limits.max_memory_bytes = 33554432`（32MB），作为 QuickJS 运行时内存上限（`crates/bifrost-script/src/sandbox.rs:99`）
- `sandbox.limits.max_decode_input_bytes = 2097152`（2MB），decode 输入 bytes 上限，超过会跳过 decode（防止性能/内存风险）
- `sandbox.limits.max_decompress_output_bytes = 10485760`（10MB），HTTP body 解压输出上限，超过会放弃解压并回退到原始压缩数据（防止压缩炸弹）
- `sandbox.file.max_bytes = 1048576`（1MB），单次 `file.readText/writeText/appendText` 读写上限
- `sandbox.net.timeout_ms = 5000`（5s），`net.fetch` 单次请求超时
- `sandbox.net.max_request_bytes = 262144`（256KB），`net.fetch` 请求体上限
- `sandbox.net.max_response_bytes = 1048576`（1MB），`net.fetch` 响应体上限

限制含义（建议在写脚本前确认）：

- **时间限制**：命中 `timeout_ms` 会强制终止脚本（常见原因：对大字符串/大 JSON 做复杂处理）。
- **内存限制**：超过 `max_memory_bytes` 会触发 QuickJS 内存限制，脚本会失败。
- **decode 输入限制**：超过 `max_decode_input_bytes` 会跳过 decode，避免对超大 payload 做解码。
- **解压输出限制**：超过 `max_decompress_output_bytes` 会放弃解压，避免压缩炸弹导致内存/CPU 风险。
- **文件限制**：相对路径禁止 `..`，绝对路径仅能访问 `allowed_dirs`；读写大小受 `file.max_bytes` 限制。
- **网络限制**：仅允许 `http/https`；请求/响应体大小与超时分别受 `net.*` 限制。

### 5.2 安全建议

- 不要把敏感凭证硬编码在脚本里；优先放到 Values，再通过 `ctx.values[...]` 读取。
- 尽量避免在脚本里发起外部网络请求；如需使用，限制域名并收紧 `sandbox.net.*`。
- 对二进制/大包处理优先用 `bodyHex` 与截断标记，避免对超大字符串做复杂操作导致超时。
