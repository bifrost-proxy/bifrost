# SSE / WS 数据持久化与性能优化

## 现状结论

这份文档部分内容已经实现，但需要把目标态与现状拆开理解。

## 已落地部分

- WebSocket payload 已切换到 `WsPayloadStore`，由按帧独立文件改为按连接追加文件 + `BodyRef::FileRange`。
- `/frames/{frame_id}` 详情读取时仍向前端返回字符串；二进制帧按需要转成 Base64。
- `/traffic/{id}/sse/stream` 已存在，SSE 展示链路已经从 WebSocket frames 中抽离出来。

## 尚未适合作为“已实现事实”的部分

- 文档里关于 SSE write-behind buffer 的 64KB/200ms 刷盘策略 (planned, not yet shipped as of 2026-06-17)：当前实际配置项为 `sse_stream_flush_bytes`（默认 256KB）与 `sse_stream_flush_interval_ms`（默认 1000ms），见 `crates/bifrost-admin/src/handlers/config.rs`。
- “SSE 全面不再产生 frames” 也不应写成绝对事实；真实情况是 SSE 详情页主路径已经独立（`/api/traffic/{id}/sse/stream`），但底层代码中仍能看到部分 SSE frame 记录逻辑。

## 当前更准确的描述

- WebSocket：本设计的大部分方向已经成为现实实现。
- SSE：当前仓库属于“专用流接口已落地，底层完全清理旧 frame 语义仍未完全收口”的过渡状态。

## 建议

- 后续如果继续维护这份文档，建议拆成两篇：
  - `websocket payload append store`：记录已经落地的 FileRange 存储方案；
  - `sse raw stream persistence`：单独说明 SSE 仍在收敛中的写入/广播实现。
