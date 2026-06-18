# Capture Wait API & CLI

## 目标

给 Bifrost 增加一条"等待并捕获下一条匹配请求"的能力：

- 调用方提交一个简短的过滤条件（host/method/path 子串），服务端在该条件命中下一条新到达流量时立即返回该记录；若 timeout 内未命中则返回 `matched=false`。
- 通过单条 HTTP POST + ndjson 风格的 JSON 响应实现，调用方拿到响应即可结束等待，无需维护长连接订阅状态。
- 在 CLI 上暴露 `bifrost capture wait`，并支持 `--open <url>` 在等待前打开浏览器/系统默认应用，触发用户操作。

## 非目标

- 不替代 `/api/push` 长连接订阅；本能力专门为"等待某一条请求"的一次性命中场景设计。
- 当前 P0 实现 **不** 接入 SearchEngine 的 JSONPath 过滤（`req_json` 留接口但实现里恒返回 true，等 P0-2 合入后整合）。
- 不做 body 脱敏 / 大 body 转储；返回的 `record` 字段直接取 `TrafficRecord`，调用方需把输出视为敏感数据。

## 数据流

```
proxy 真实流量
   │ (TrafficStoreEvent::Inserted)
   ▼
traffic_db_store.subscribe()  ─── broadcast::Receiver<TrafficStoreEvent>
   │
   ▼
PushManager::subscribe_once(matcher, timeout)
   │  内部 spawn 一个独立 tokio task：
   │   - 持有 receiver
   │   - 收到 Inserted/Updated → 把 record 转成 TrafficSummaryCompact，先跑 matcher，
   │     命中则 send 到 oneshot，并 break。
   │   - select! 上挂 tokio::time::timeout
   │
   ▼ (Some(record) 或 None)
HTTP handler 把命中 record 通过 store.get_by_id() 拿到完整 TrafficRecord，
组装 JSON 响应。
```

不与 PushManager 内部的 `broadcast_traffic_events` 流抢资源：`subscribe_once` 在 `traffic_db_store.subscribe()` 上开一个独立 receiver，broadcast channel 天然支持多消费者；匹配器在 spawn 的 task 里跑，命中或超时后 task 退出，receiver drop，无 leak。

## API 契约

`POST /_bifrost/api/capture/wait`

请求 body：

```json
{
  "host_contains": "bits.bytedance.net",
  "method": "POST",
  "path_contains": "/api/widget",
  "req_json": [{"path": "$.chart_name", "value": "Commit基本信息"}],
  "timeout_ms": 120000
}
```

字段说明：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `host_contains` | string | 否 | host 子串匹配，大小写不敏感 |
| `method` | string | 否 | 精确大写匹配，如 `POST` |
| `path_contains` | string | 否 | path 子串匹配 |
| `req_json` | array | 否 | P0 阶段只做接口占位，恒返回 true；P0-2 合入后接 SearchEngine |
| `timeout_ms` | u64 | 否 | 等待超时，默认 60000，最大 600000 |

响应（始终 HTTP 200）：

命中：
```json
{ "matched": true, "record": { ...完整 TrafficRecord 字段... }, "waited_ms": 3210, "scanned_count": 5 }
```

超时：
```json
{ "matched": false, "scanned_count": 12, "waited_ms": 60000 }
```

`scanned_count` 反映等待期间真正被 matcher 评估过的新流量数。

## PushManager 增量

只新增一个对外方法，不改任何现有行为：

```rust
impl PushManager {
    pub async fn subscribe_once(
        self: &Arc<Self>,
        matcher: impl Fn(&TrafficSummaryCompact) -> bool + Send + Sync + static,
