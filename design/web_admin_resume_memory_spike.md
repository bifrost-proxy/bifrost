# Web 管理端睡眠恢复与内存放大

## 现状结论

旧文档定位的问题基本正确，但很多“待实施措施”现在已经实现了，文档需要从“方案”改成“现状说明”。

## 已实现修复

- Push 发送队列已经从无界改成有界 `mpsc::channel(PUSH_CHANNEL_CAPACITY)`。
- `PushClient::send()` 已使用 `try_send`，慢客户端会被自然淘汰，而不是无限堆积。
- WebSocket 连接已加入协议层 `Ping / Pong`。
- 服务端已支持基于 `x_client_id` 做客户端分桶与淘汰。
- 订阅侧对 `pending_ids` 已有限额，`websocket.rs` 中会裁剪到 `MAX_SUBSCRIBED_IDS`。
- 前端 `useTrafficStore` 已增加：
  - `MAX_PENDING_IDS = 500`
  - `POLL_MIN_INTERVAL = 200`
  - `HAS_MORE_BACKOFF_INTERVAL = 500`
- 页面隐藏时会暂停 `traffic` 与 `metrics` 的实时 push，恢复时先恢复 `traffic` 再恢复 `metrics`。

## 隐藏后恢复的补量策略

- `traffic` 列表恢复时必须重新建立 `/api/push` 连接，而不是沿用隐藏前的连接状态。
- 隐藏阶段需要显式断开底层 `pushService` 连接，避免被其他仍持有 ref 的订阅者继续把旧连接保活。
- 重连时需要让 `need_traffic` 和 `last_sequence` 出现在建连阶段的订阅快照中，这样服务端 `send_initial_traffic_delta` 才会按 backlog 批量补发 `traffic_delta`。
- 恢复顺序必须是：
  - 先 `useTrafficStore.enablePush()`
  - 再 `useMetricsStore.enablePush()`
- 如果先恢复 `metrics`，WebSocket 会以“只有 metrics 订阅”的快照先连上，`traffic` 只能在 open 后靠二次订阅补上，服务端就拿不到首个批量补量窗口。

## 仍需谨慎的点

- 前端目前仍以 `pending_ids` 模型为主，没有实现文档里提到的 `visible_ids` 方案（planned, not yet shipped as of 2026-06-17）。
- 多 tab 复用单 push 连接也还不是当前实现（planned, not yet shipped as of 2026-06-17）。

## 结论

- 这份文档不应继续把“bounded queue / ping pong / client bucket / pending_ids 限额”写成待做事项。
- 当前更准确的定位是：核心防护已经落地，剩余的是进一步降低恢复瞬时流量的增量优化。
