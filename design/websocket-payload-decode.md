# WebSocket Payload Decode

## 背景

当前 WebSocket 帧在管理端展示与搜索时：

- Text/Close/Sse 使用 UTF-8 lossy 展示与入库
- Binary/Ping/Pong/Continuation 使用 base64 展示与入库

在 TLS 解包/MITM 场景连接恢复后，用户希望像浏览器一样看到“解码后的二进制消息”，并且解码结果需要可搜索（即用于搜索的文本应写入入库字段，而不是仅前端临时转换）。

## 目标

- 支持对 WebSocket frame payload 做“落库前解码”，解码结果参与：
  - frames 列表预览
  - frames 详情 full_payload
  - 搜索 websocket_messages
- 支持默认解码器与用户自定义解码脚本：
  - `decode://utf8`：内置 UTF-8 解码器（lossy）
  - `decode://<script>`：使用 scripts/decode 目录下的 JS decode 脚本
- 保留原始 payload 的可用信息（至少在 UI/接口层可回溯 raw/base64 形式）

## 非目标

- 不尝试自动识别 protobuf/msgpack 等协议并内置解析器
- 不改变 WebSocket 转发数据通路（不对网络转发 payload 做修改）
- 不保证对历史已落库数据的兼容（协议升级后可重建数据库）

## 规则与配置

- 复用现有规则协议 `decode://...`（ResolvedRules.decode_scripts）
- 新约定：
  - `decode://utf8` 与 `decode://default` 作为内置解码器标识
  - 其它名称走 JS decode 脚本执行

## 执行时机与数据流

### 执行时机

- 在代理侧 WebSocket 双向转发循环中，在 `connection_monitor.record_frame(...)` 之前进行：
  1. 可选 permessage-deflate 解压（现有逻辑）
  2. decode:// 解码（新增逻辑）
  3. 仅将“解码后的文本”写入 payload_preview/payload_ref（用于展示与搜索）
  4. 将“原始 payload”的引用写入 raw_payload_ref/raw_payload_preview（用于回溯）

### decode 脚本执行

- 复用 bifrost-script 的 Decode 脚本执行接口
- 为 WebSocket 方向提供 phase：
  - Send：`websocket_send`（payload 作为 request_body_bytes）
  - Receive：`websocket_recv`（payload 作为 response_body_bytes）
- 通过 ScriptContext.values 注入 ws 元信息：
  - `ws_direction` / `ws_frame_type` / `ws_payload_size`

## 存储与搜索

- frames 搜索仍复用现有逻辑：优先查 `payload_preview`，再按 `payload_ref` 加载完整内容做 substring 搜索
- 关键变化：当 decode 生效时，`payload_preview/payload_ref` 写入的是“解码后的文本”，因此搜索天然命中

## API 变更

在 WebSocketFrameRecord 中新增字段：

- `payload_is_text: bool`：决定 full_payload 的展示编码（utf8/base64）
- `raw_payload_preview?: string`
- `raw_payload_ref?: BodyRef`
- `raw_payload_size?: usize`
- `raw_payload_is_text?: bool`：当存在 raw payload 时，标记其是否为文本（用于决定 raw 视图的解码方式）

frames list/detail 会携带这些字段，前端默认展示 decoded payload（payload_preview/full_payload），需要时可展示 raw/base64。

## 失败与降级策略

- 未配置 decode://：保持现有行为（binary 以 base64 展示与入库）
- 配置了 decode:// 但脚本不可用/执行失败：降级为内置 utf8 lossy（若包含 utf8/default），否则保持现有行为
- payload 过大：复用 HTTP decode 的输入大小保护，超阈值跳过脚本执行，仍可做 utf8 lossy（可配置为完全跳过）

## 影响范围

- bifrost-proxy：WebSocket 转发循环增加 decode 执行
- bifrost-admin：
  - WebSocketFrameRecord 扩展字段
  - frames detail full_payload 编码逻辑改为使用 payload_is_text
  - /api/syntax 补充 decode scripts 列表（含内置 utf8/default）
- web：admin 前端 Messages 面板使用 payload_is_text 透传字段（无需强制改动，但可增强 raw/decoded 切换）

## 测试

- 单元测试：decode negotiation / payload encoding
- e2e：新增 WebSocket binary payload decode 用例：
  - 通过规则开启 `decode://utf8`
  - ws_stress_client 发送 binary bytes（有效 UTF-8）
  - 断言 frames detail full_payload 直接返回文本且搜索可命中关键词

