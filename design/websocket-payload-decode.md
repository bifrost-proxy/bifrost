# WebSocket Payload Decode 设计方案

## 背景

Bifrost 抓取 WebSocket frame 时，出于协议中立要写入 frame store 与推送给管理端：

- Text / Close / Sse：`String::from_utf8_lossy` 展示与入库
- Binary / Ping / Pong / Continuation：base64 展示与入库

在 TLS 解包 / MITM 场景下，用户经常需要像浏览器一样看到 "解码后的二进制消息" 而不是 base64 密文；并且解码结果需要参与 `websocket_messages` 全文搜索——所以必须在落库前完成 decode，而不是仅在前端临时反显。

Bifrost 的规则语言里已经存在 `decode://<script>` 协议（HTTP body 用），本方案把它复用到 WebSocket 帧上：

- `decode://utf8` 与 `decode://default` 为内置 UTF-8 lossy decoder。
- `decode://<script>` 走 `crates/bifrost-script` 里的 JS decode 脚本。
- 复用 `ResolvedRules.decode_scripts` 字段，无需扩展规则语法。

## 用户目标验证清单

### 必须实现

- 支持 WebSocket frame payload 的 "落库前解码"，解码结果同时进入：
  - Frames 列表预览 (`payload_preview`)
  - Frames 详情 full_payload
  - 搜索 `/api/traffic/:id/frames/search` 与全局 `websocket_messages` 搜索
- 内置 decoder：`decode://utf8` / `decode://default`（互为别名，均为 UTF-8 lossy）。
- 用户自定义 decoder：`decode://<script>`，脚本位置由 `bifrost-script` 引擎解析。
- Frame 转发数据通路不变（不改变发出/收到的 payload）。
- 原始 payload 仍可在 UI/API 上回溯（base64 或 raw text）。

### 必须不破坏

- 未配置 `decode://` 时：现有 Binary/Ping/Pong/Continuation → base64，Text/Close/Sse → UTF-8 lossy 的行为不变。
- 内置 `permessage-deflate` 解压 (`crates/bifrost-proxy/src/protocol/websocket/deflate.rs`) 在 decode 之前完成，两者顺序确定。
- WebSocket 握手 header 规则、frame 转发、连接监控、body_store 引用不受影响。
- HTTP body 上的 `decode://` 行为不受影响；`crates/bifrost-proxy/src/proxy/http/handler/decode.rs` 与 `ws_decode.rs` 保持独立分派。

### 必须真实验证

- E2E 加载 `e2e-tests/rules/websocket/decode_utf8_searchable.txt`，`ws_stress_client` 发送 binary bytes（有效 UTF-8）后，frames detail full_payload 返回文本、search 命中关键词。
- Frame record 的 `payload_is_text` / `raw_payload_*` 字段真实写入，`crates/bifrost-admin/src/connection_monitor.rs` 与 `crates/bifrost-admin/src/frame_store.rs` 均按新语义读取。
- Admin `/api/syntax` 列出内置 `utf8` 与 `default` 两个 decoder 且被前端 Monaco 补全 (`web/src/components/BifrostEditor/snippet/syntaxApi.ts`) 提示。

## 非目标

- 不自动识别 protobuf / msgpack / MQTT / Socket.IO 等二进制协议并内置解析器。
- 不改变 WebSocket 双向转发的实际字节。
- 不保证对协议升级前历史 frames 的兼容；升级后若数据库 schema 有变化，可重建库。

## 产品语义

### 规则协议

- 完全复用 `decode://` 协议，字段落到 `ResolvedRules.decode_scripts: Vec<String>`。
- `decode://utf8`、`decode://default` 为保留名称，直接走内置 UTF-8 lossy 实现，不需要注册 JS 脚本。
- 其它名称走 JS decode 脚本；脚本环境由 `crates/bifrost-script/src/engine.rs` 提供。

### 阶段（phase）

WebSocket decode 与 HTTP request/response decode 共享同一套 `ScriptContext` 但用不同 `phase`：

- `websocket_send`：客户端 → 服务端方向；payload 通过 `request.bodyBase64` / `request.body` 暴露；`response === null`。
- `websocket_recv`：服务端 → 客户端方向；payload 通过 `request.bodyBase64` / `request.body` 暴露；`response === null`。

这一 mental model 与 `assets/scripts/parser/build_in_bp.js` 中的处理一致，也与 `web/src/pages/Scripts/index.tsx` 里 `BifrostScriptPhase = "request" | "response" | "websocket_send" | "websocket_recv"` 的类型定义一致。

### `ScriptContext.values` 注入

脚本可通过 `ctx.values` 读取本次帧的元信息：

- `ws_direction`：`"send"` / `"receive"`
- `ws_frame_type`：`"text"` / `"binary"` / `"ping"` / `"pong"` / `"close"` / `"continuation"`
- `ws_payload_size`：解压后 payload 字节数

## 执行时机与数据流

### 执行位置

代码集中在 `crates/bifrost-proxy/src/proxy/http/ws_decode.rs`：

- `decode_ws_payload_for_storage(...)`：接收解压后的 raw payload、`decode_scripts`、`WsHandshakeMeta`、`FrameDirection`，返回落库用 payload。
- `is_builtin_decoder(name)`：`matches!(name, "utf8" | "default")`。
- `builtin_decode_utf8(input)`：`String::from_utf8_lossy(input).to_string().into_bytes()`。

调用点在 WebSocket 双向 forwarder（`crates/bifrost-proxy/src/protocol/websocket/forwarder.rs`）→ `crates/bifrost-proxy/src/proxy/http/websocket/capture.rs::record_frame` 之前：

1. 解压 `permessage-deflate`（已有）
2. 调用 `decode_ws_payload_for_storage`（新增）
3. 写入 `payload_preview` / `payload_ref` = 解码后文本
4. 写入 `raw_payload_size` / `raw_payload_is_text` / `raw_payload_preview` / `raw_payload_ref` = 原始 payload

`capture.rs` 内部拆出两个 clone：`scripts_c2s = decode_scripts.clone()` 与 `scripts_s2c = decode_scripts;`，方向独立，避免 borrow 冲突。

### FrameDirection → phase 映射

`ws_decode.rs`（第 191-192 行）：

```rust
match direction {
    FrameDirection::Send => "websocket_send",
    FrameDirection::Receive => "websocket_recv",
}
```

## 存储与搜索

`crates/bifrost-admin/src/connection_monitor.rs` `WebSocketFrameRecord` 已经承载新字段：

```rust
pub struct WebSocketFrameRecord {
    // ...
    pub payload_is_text: bool,
    pub payload_preview: Option<String>,
    pub payload_ref: Option<BodyRef>,
    pub raw_payload_size: Option<usize>,
    pub raw_payload_is_text: Option<bool>,
    pub raw_payload_preview: Option<String>,
    pub raw_payload_ref: Option<BodyRef>,
}
```

搜索路径 (`crates/bifrost-admin/src/frame_store.rs`)：

- 优先查 `f.payload_preview`（第 763 行）。
- 命中失败再检查 `f.raw_payload_preview` / `f.payload_ref` / `f.raw_payload_ref`。
- 因为 decode 生效时 `payload_preview` = decoded text，所以搜索天然命中解码后关键词。

## API 变更

### Frames Admin API

- `GET /_bifrost/api/traffic/:id/frames`（`crates/bifrost-admin/src/handlers/frames.rs`）：返回列表条目携带 `payload_is_text` 与 `raw_payload_*` 字段。
- `GET /_bifrost/api/traffic/:id/frames/:frame_id`：返回 detail，包含解码后 full_payload（通过 body_store 反查 `payload_ref`）与原始 payload 引用（`raw_payload_ref`）。
- `GET /_bifrost/api/syntax`（`crates/bifrost-admin/src/handlers/syntax.rs`）：`decode_scripts` 列表内置 `utf8` 与 `default` 两条 (`ScriptListItem`)，后续追加用户 JS decode 脚本。前端 Monaco snippet 直接消费。

### 前端消费

- `web/src/components/TrafficDetail/panes/Messages/index.tsx` 通过 `payload_is_text` 决定 full_payload 展示编码。
- Raw / decoded 切换 UI 可后续增强，但存储层字段已就绪。

## 失败与降级策略

- **未配置 decode://**：跳过 `decode_ws_payload_for_storage`，保持现有行为（binary → base64）。
- **配置 decode:// 但脚本不可用 / 执行失败**：
  - 若配置中包含 `utf8` 或 `default`，降级为 `builtin_decode_utf8`。
  - 否则退回原始 payload，`raw_payload_*` 仍写入以便 UI 回溯。
- **payload 过大**：`decode_ws_payload_for_storage` 与 HTTP decode 共享上限保护；超过阈值直接跳过脚本，日志中打印 `[DECODE][WS] skip decode ({} bytes > {} limit)`（`ws_decode.rs:90`）。仍可选择跑内置 utf8 lossy，或完全跳过（按配置）。
- **UTF-8 非法字节**：`String::from_utf8_lossy` 已做 `U+FFFD` 替换，不 panic。

## 影响范围

- `crates/bifrost-proxy`：
  - `src/proxy/http/ws_decode.rs`（新逻辑集中于此）
  - `src/proxy/http/websocket/capture.rs`（transfer decode_scripts 到 forwarder）
  - `src/proxy/http/websocket/mod.rs`（把 `ws_rules.decode_scripts.clone()` 透传给 forwarder）
  - `src/protocol/websocket/forwarder.rs`（在 `record_frame` 前调用 decode）
- `crates/bifrost-admin`：
  - `src/connection_monitor.rs`（`WebSocketFrameRecord` 字段扩展）
  - `src/frame_store.rs`（search / 内存分账考虑 raw payload）
  - `src/handlers/frames.rs`（list/detail 序列化新字段）
  - `src/handlers/syntax.rs`（内置 `utf8` / `default` 出现在 `decode_scripts`）
- `crates/bifrost-script`：`ScriptContext` 支持 `websocket_send` / `websocket_recv` 两个 phase。
- `web`：`Scripts` 页面类型 (`web/src/pages/Scripts/index.tsx`) 与 Monaco snippet (`syntaxApi.ts`) 已覆盖新 phase。

## CLI + Web + Admin API 边界

### CLI

- 不新增 CLI 参数。用户通过 `bifrost rule update <name> --content "* decode://utf8"` 或 `--file` 打开 WS decode。
- 也可用 `bifrost script <subcommand>` 管理 JS decode 脚本（复用现有 script CLI）。

### Web UI

- Rules Editor：`decode://utf8`、`decode://default` 自动补全，来自 `/api/syntax`。
- Scripts 页面：可创建 / 编辑 decode 脚本；页面自身提示 `websocket_send` / `websocket_recv` 的沙箱语义。
- Traffic Detail Messages 面板：默认展示 decoded payload；后续可增加 raw / decoded 切换按钮。

### Admin API

- `GET /_bifrost/api/syntax`：`decode_scripts` 出现内置 `utf8` / `default`。
- `GET /_bifrost/api/traffic/:id/frames`：新字段 `payload_is_text` / `raw_payload_*`。
- `GET /_bifrost/api/traffic/:id/frames/:frame_id`：full_payload 编码由 `payload_is_text` 决定。
- 搜索：全局与单会话搜索均命中 `payload_preview`（decoded 文本）。

## Sync 边界

- WebSocket decode 脚本以普通 Bifrost script 存在，`bifrost script` sync 走现有 Group / Personal Sync 通道；本设计不改变 Sync schema。
- Frame 数据本身不参与 Sync；不同设备的 frame store 相互独立。

## Phase 拆分

### Phase 1：内置 UTF-8 decoder + storage 字段扩展

- `WebSocketFrameRecord` 新增 `payload_is_text` / `raw_payload_*`。
- `frame_store.rs` 序列化 / 搜索 / 内存分账兼容新字段。
- Admin `/api/syntax` 补 `utf8` / `default` 两个内置 decoder。
- 单元测试：`is_builtin_decoder_matches_exact_names_only`、`builtin_decode_utf8_roundtrips_and_is_lossy`。

### Phase 2：Proxy forwarder 接入

- `capture.rs` 增加 `decode_scripts` 参数并 clone 给双向 forwarder。
- `forwarder.rs` 在 `record_frame` 前调用 `decode_ws_payload_for_storage`。
- 单元测试：`decode_ws_payload_returns_none_for_empty_inputs_and_missing_state`、`decode_ws_payload_applies_builtin_utf8_decoder`、`decode_ws_payload_skips_large_payload_with_marker`。

### Phase 3：Script engine + phase 支持

- `bifrost-script` 增加 `websocket_send` / `websocket_recv` 两个 phase。
- `ScriptContext.values` 注入 `ws_direction` / `ws_frame_type` / `ws_payload_size`。
- 前端 `Scripts` 页面文档与类型跟进。

### Phase 4：Admin/前端 + 文档

- Frames list/detail API 返回新字段。
- Messages 面板按 `payload_is_text` 展示。
- 更新 `human_tests/proxy-websocket-sse.md` 与 `human_tests/webui-search.md`。
- `site/src/content/docs/reference/rules/scripts.md` 与 `docs/rules/scripts.md` 补 `websocket_send` / `websocket_recv` 用法。

## 测试方案

### 单元测试

`crates/bifrost-proxy/src/proxy/http/ws_decode.rs` 中已经实现的用例：

- `is_builtin_decoder_matches_exact_names_only`（`utf8` / `default` 精确匹配，空格或未知名字均 false）。
- `builtin_decode_utf8_roundtrips_and_is_lossy`（ASCII 保留、非法字节走 `U+FFFD` 替换）。
- `decode_ws_payload_returns_none_for_empty_inputs_and_missing_state`（未配置 decode 或 payload 空返回 None）。
- `decode_ws_payload_skips_large_payload_with_marker`（超过阈值只写 marker）。
- `decode_ws_payload_applies_builtin_utf8_decoder`（真正走内置解码路径并回写 `payload_preview`）。

### E2E

- `e2e-tests/tests/test_websocket_frames.sh` 加载 `e2e-tests/rules/websocket/decode_utf8_searchable.txt`（`* decode://utf8`）：
  - `ws_stress_client` 发送 binary bytes（有效 UTF-8）。
  - 断言 frames detail full_payload 返回文本而非 base64。
  - 断言 frame search 命中关键词。
- `e2e-tests/tests/test_frames_admin_api.sh`：验证 `GET /api/traffic/:id/frames` 与 `frames/:id` 响应携带新字段。

### 真实场景测试

- `human_tests/proxy-websocket-sse.md::TC-PWS-02` / `TC-PWS-05`：确认 Messages 面板在 `decode://utf8` 生效时显示解码后文本，且 Frames 全文搜索 (`human_tests/webui-search.md`) 命中。
- `human_tests/webui-scripts.md`：新增 / 更新 WebSocket decode 脚本创建流程；确保 `websocket_send` / `websocket_recv` 沙箱行为文档化。

## Review / Fix / Test 闭环

### 第 1 轮

- 复查用户目标：内置 utf8 生效、JS decode 生效、原始 payload 可回溯。
- 复查 diff：`ws_decode.rs`、`capture.rs`、`upgrade.rs`、`connection_monitor.rs`、`frame_store.rs`、`handlers/frames.rs`、`handlers/syntax.rs`、`bifrost-script/src/engine.rs`、前端 `Scripts/index.tsx`、`site` 文档。
- 复测：
  - `cargo test -p bifrost-proxy ws_decode`
  - `cargo test -p bifrost-admin frame_store`
  - `cargo test -p bifrost-admin syntax`
  - `BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_websocket_frames.sh`

### 第 2 轮

- 复查第 1 轮修复；确认 `payload_ref` 与 `raw_payload_ref` 都真正落到 `body_store`，不出现只写内存的降级。
- 复跑 `e2e-tests/tests/test_frames_admin_api.sh` 与 `e2e-tests/tests/test_bp_parser_e2e.sh`（BP 脚本 phase）。
- 复审前端 Monaco 补全是否成功列出 `utf8` / `default`（`web/tests/ui/admin-scripts.spec.ts`）。

## 校验要求

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-proxy ws_decode`
- `cargo test -p bifrost-admin frame_store`
- `cargo test -p bifrost-admin syntax`
- `BIFROST_BIN=target/release/bifrost bash e2e-tests/tests/test_websocket_frames.sh`
- `bash e2e-tests/tests/test_frames_admin_api.sh`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

如本机遵循 no-local-coverage 约定，不本地执行 `make coverage` / `make coverage-unit`；交付时说明豁免并依赖远端 CI。

## 文档更新要求

- `site/src/content/docs/reference/rules/scripts.md` 与 `docs/rules/scripts.md`：`websocket_send` / `websocket_recv` phase 用法与 `request.bodyBase64` 沙箱说明。
- `human_tests/proxy-websocket-sse.md` 与 `human_tests/webui-search.md`：解码后搜索命中的真实用例。
- `human_tests/webui-scripts.md`：新建 decode 脚本流程。
- `README` / CLI help 不需要用户可见改动。

## 风险与决策点

- **内置名称冲突**：`utf8` / `default` 是保留名，不允许被普通用户 JS 脚本覆盖。`is_builtin_decoder` 精确匹配（不含空格），Admin API 也必须按同一规则过滤，避免用户创建同名脚本导致命中歧义。
- **payload 大小限制**：decode 脚本执行有上限（复用 HTTP decode 阈值），超限时应写清晰 marker 并让前端展示 "payload too large"，而不是静默返回空。
- **原始 payload 保留策略**：`raw_payload_*` 由 body_store 引用；如果 body_store 分账压力大，需要单独评估 `raw_payload_ref` 是否可回退到只保 preview（前 N 字节）。当前实现两者都保留。
- **Phase 沙箱**：`websocket_send` / `websocket_recv` 阶段 `response === null`，脚本必须通过 `request.bodyBase64` / `request.body` 读取；文档若不明说，用户容易误写 `response.bodyBase64` 导致返回 undefined。已在 `site/src/content/docs/reference/rules/scripts.md:90-91` 与 `Scripts/index.tsx:69-70` 显式提示。
- **数据库不做 migration**：升级 schema 后旧 frames 可能缺 `raw_payload_*`。当前策略是接受重建 body_store；如果未来要平滑迁移，需要单独设计 backfill。
- **与 HTTP decode 的独立性**：`ws_decode.rs` 与 `handler/decode.rs` 明确分文件；不要把 HTTP decode 与 WS decode 合并成同一入口，避免 phase 判定被合并出 bug。
