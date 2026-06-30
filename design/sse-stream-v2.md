## 现状结论

这份设计已经基本落地，当前流量详情页的 SSE 路径确实使用独立的 `/traffic/{id}/sse/stream`，不再依赖 `/frames/{id}` 回补首屏消息。

## 当前实现

- 前端打开中的 SSE 连接会建立：
  - `EventSource /traffic/{id}/sse/stream?from=begin&batch=1`
- 已结束的 SSE 连接则直接读取 `responseBody`，在前端本地解析为事件列表。
- 消息面板与正文面板共享同一份原始流：
  - 消息列表消费结构化事件；
  - `raw` 字段会继续 append 到 `useTrafficStore.responseBody`，保证正文持续增长。

## 与原设计的差异

- 旧文档中“完全取消 SSE frames 入库”的表述不应再视为仓库级事实；当前流量详情主路径已经解耦，但仓库里仍能看到部分 SSE frame 记录逻辑在其他链路中存在。
- 真正已经稳定落地的是“详情页主消费路径改成 SSE 专用流 + response body 解析”，而不是“全仓库彻底删除 SSE frame 语义”。

## 文档结论

- 如果讨论 Traffic Detail 的 SSE 展示链路，本方案已经是当前真实实现。
- 如果讨论底层存储是否彻底去掉 SSE frame，需要单独做更细的设计核对，不能直接沿用旧文档中的绝对表述。

## 回归测试要求

- `crates/bifrost-e2e` 的 `replay_sse_live_stream_keeps_tail_events` 和 `replay_sse_live_stream_keeps_done_event` 必须验证 live 详情流不会丢失尾部事件、OpenAI 风格 `[DONE]` 事件和 synthetic finish。
- 测试客户端读取 `/_bifrost/api/traffic/{id}/sse/stream?from=begin&batch=1` 时应以“已收到目标事件数 + 完成标记”为验证边界，达到边界后主动关闭订阅，不能依赖管理端 SSE HTTP 连接一定在平台上及时 EOF；否则 Windows CI 可能在功能已经满足时等待到 per-test timeout。
- 对应 human_tests 用例为 `human_tests/api-traffic.md` 的 TC-ATR-24。
