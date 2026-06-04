# Bifrost Admin API 文档

本文档详细描述了 Bifrost 代理服务的管理端 API 接口。

## 概述

- **路由前缀**: `/_bifrost`
- **API 前缀**: `/_bifrost/api/`
- **公开接口前缀**: `/_bifrost/public/`
- **静态文件**: `/_bifrost/` 下的非 API 路径提供 Web UI 静态文件

### CORS 支持

所有 API 接口支持 CORS 跨域请求：

- `Access-Control-Allow-Origin: *`
- `Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS`
- `Access-Control-Allow-Headers: Content-Type, Authorization, X-Client-Id`

### 响应格式

所有 API 响应均为 JSON 格式，Content-Type 为 `application/json`。

**成功响应示例**:

```json
{
  "success": true,
  "message": "操作成功"
}
```

**错误响应示例**:

```json
{
  "error": "错误信息",
  "status": 400
}
```

---

## 1. Rules API - 规则管理

用于管理代理规则文件，支持规则的增删改查和启用/禁用。

### 1.1 获取规则列表

```
GET /api/rules
```

**响应**:

```json
[
  {
    "name": "default",
    "enabled": true,
    "rule_count": 10
  }
]
```

| 字段       | 类型    | 说明                           |
| ---------- | ------- | ------------------------------ |
| name       | string  | 规则文件名称                   |
| enabled    | boolean | 是否启用                       |
| rule_count | number  | 有效规则数量（排除空行和注释） |

**应用场景**: Web UI 展示规则列表，显示每个规则文件的状态。

### 1.2 创建规则

```
POST /api/rules
```

**请求体**:

```json
{
  "name": "my-rules",
  "content": "example.com proxy://127.0.0.1:8080",
  "enabled": true
}
```

| 字段    | 类型    | 必填 | 说明                |
| ------- | ------- | ---- | ------------------- |
| name    | string  | 是   | 规则文件名称        |
| content | string  | 是   | 规则内容            |
| enabled | boolean | 否   | 是否启用，默认 true |

**响应**: 成功响应

**应用场景**: 通过 API 或 Web UI 创建新的规则文件。

### 1.3 获取规则详情

```
GET /api/rules/{name}
```

**路径参数**:

- `name`: 规则文件名称（需 URL 编码）

**响应**:

```json
{
  "name": "default",
  "content": "example.com proxy://127.0.0.1:8080\n*.test.com file:///path/to/mock.json",
  "enabled": true
}
```

**应用场景**: 编辑规则时获取规则内容。

### 1.4 更新规则

```
PUT /api/rules/{name}
```

**路径参数**:

- `name`: 规则文件名称

**请求体**:

```json
{
  "content": "新的规则内容",
  "enabled": true
}
```

| 字段    | 类型    | 必填 | 说明     |
| ------- | ------- | ---- | -------- |
| content | string  | 否   | 规则内容 |
| enabled | boolean | 否   | 是否启用 |

**应用场景**: 修改规则内容或状态。

### 1.5 删除规则

```
DELETE /api/rules/{name}
```

**路径参数**:

- `name`: 规则文件名称

**应用场景**: 删除不再需要的规则文件。

### 1.6 启用规则

```
PUT /api/rules/{name}/enable
```

**应用场景**: 快速启用指定规则。

### 1.7 禁用规则

```
PUT /api/rules/{name}/disable
```

**应用场景**: 临时禁用规则而不删除。

---

## 2. Traffic API - 流量管理

用于查看和管理代理流量记录，支持 WebSocket 和 SSE 连接的帧数据查看。

### 2.1 获取流量列表

```
GET /api/traffic
```

**查询参数**:

| 参数                     | 类型    | 说明                   |
| ------------------------ | ------- | ---------------------- |
| method                   | string  | HTTP 方法过滤          |
| status                   | number  | 精确状态码             |
| status_min               | number  | 最小状态码             |
| status_max               | number  | 最大状态码             |
| url / url_contains       | string  | URL 包含               |
| host                     | string  | 主机名包含             |
| domain                   | string  | 域名包含               |
| path / path_contains     | string  | 路径包含               |
| content_type             | string  | 响应 Content-Type 包含 |
| request_content_type     | string  | 请求 Content-Type 包含 |
| protocol                 | string  | 协议 (http/https)      |
| has_rule_hit             | boolean | 是否命中规则           |
| header / header_contains | string  | 请求头或响应头包含     |
| client_ip                | string  | 客户端 IP 包含         |
| client_app               | string  | 客户端应用名称包含     |
| limit                    | number  | 返回数量限制，默认 100 |
| offset                   | number  | 分页偏移量             |

**响应**:

```json
{
  "total": 1000,
  "offset": 0,
  "limit": 100,
  "records": [
    {
      "id": "req-123",
      "sequence": 1,
      "timestamp": 1700000000000,
      "method": "GET",
      "url": "https://example.com/api",
      "status": 200,
      "content_type": "application/json",
      "request_size": 100,
      "response_size": 500,
      "duration_ms": 150,
      "host": "example.com",
      "path": "/api",
      "protocol": "https",
      "client_ip": "127.0.0.1",
      "client_app": "Safari",
      "client_pid": 1234,
      "client_path": "/Applications/Safari.app",
      "has_rule_hit": true,
      "matched_rule_count": 1,
      "matched_protocols": ["proxy"],
      "is_websocket": false,
      "is_sse": false,
      "is_h3": false,
      "is_tunnel": false,
      "frame_count": 0,
      "start_time": "2024-01-01 12:00:00.000",
      "end_time": "2024-01-01 12:00:00.150"
    }
  ]
}
```

**应用场景**: Web UI 流量列表展示，支持多维度过滤。

### 2.2 增量获取流量更新

```
GET /api/traffic/updates
```

**查询参数**:

- `after_id`: 上次获取的最后一条记录 ID
- `pending_ids`: 需要更新状态的记录 ID 列表（逗号分隔）
- `limit`: 返回数量限制
- 支持所有流量过滤参数

**响应**:

```json
{
  "new_records": [...],
  "updated_records": [...],
  "has_more": false,
  "server_total": 1000
}
```

**应用场景**: 实时刷新流量列表，只获取增量数据。

### 2.3 获取流量详情

```
GET /api/traffic/{id}
```

**响应**: 完整的 `TrafficRecord` 对象，包含请求头、响应头等详细信息。

```json
{
  "id": "req-123",
  "sequence": 1,
  "timestamp": 1700000000000,
  "method": "GET",
  "url": "https://example.com/api",
  "status": 200,
  "content_type": "application/json",
  "request_size": 100,
  "response_size": 500,
  "duration_ms": 150,
  "timing": {
    "dns_ms": 10,
    "connect_ms": 20,
    "tls_ms": 30,
    "send_ms": 5,
    "wait_ms": 80,
    "receive_ms": 5,
    "total_ms": 150
  },
  "request_headers": [["Content-Type", "application/json"]],
  "response_headers": [["Content-Type", "application/json"]],
  "client_ip": "127.0.0.1",
  "client_app": "Safari",
  "client_pid": 1234,
  "client_path": "/Applications/Safari.app",
  "host": "example.com",
  "path": "/api",
  "protocol": "https",
  "is_tunnel": false,
  "is_h3": false,
  "has_rule_hit": true,
  "matched_rules": [
    {
      "pattern": "example.com",
      "protocol": "proxy",
      "value": "127.0.0.1:8080",
      "rule_name": "default",
      "raw": "example.com proxy://127.0.0.1:8080",
      "line": 1
    }
  ]
}
```

### 2.4 清空流量记录

```
DELETE /api/traffic
```

**应用场景**: 清空所有流量记录，释放内存。

### 2.5 获取请求体

```
GET /api/traffic/{id}/request-body
```

**响应**:

```json
{
  "success": true,
  "data": "请求体内容（Base64 或文本）"
}
```

### 2.6 获取响应体

```
GET /api/traffic/{id}/response-body
```

**响应**:

```json
{
  "success": true,
  "data": "响应体内容"
}
```

### 2.7 获取 WebSocket 帧列表

```
GET /api/traffic/{id}/frames
```

**查询参数**:

- `after`: 获取指定帧 ID 之后的帧
- `limit`: 返回数量限制，默认 100

**响应**:

```json
{
  "frames": [
    {
      "frame_id": 1,
      "direction": "send",
      "frame_type": "text",
      "timestamp": 1700000000000,
      "payload_preview": "Hello...",
      "payload_size": 100
    }
  ],
  "socket_status": {
    "is_open": true,
    "send_count": 10,
    "receive_count": 20,
    "send_bytes": 1000,
    "receive_bytes": 2000,
    "frame_count": 30
  },
  "last_frame_id": 30,
  "has_more": false,
  "is_monitored": true
}
```

**应用场景**: 查看 WebSocket 连接的消息帧。
> 兼容行为：对 SSE 记录，该接口会返回空 `frames`，但会尽可能返回 `socket_status/last_frame_id` 等统计信息；SSE 事件内容请使用 `/api/traffic/{id}/response-body` + `/api/traffic/{id}/sse/stream`。

### 2.8 获取帧详情

```
GET /api/traffic/{id}/frames/{frame_id}
```

**响应**: 包含完整 payload 的帧详情。

```json
{
  "frame": {
    "frame_id": 1,
    "direction": "send",
    "frame_type": "text",
    "timestamp": 1700000000000,
    "payload_preview": "Hello...",
    "payload_size": 100
  },
  "full_payload": "完整的帧内容"
}
```

### 2.9 订阅帧流 (SSE)

```
GET /api/traffic/{id}/frames/stream
```

**响应**: Server-Sent Events 流，实时推送新帧。

```
Content-Type: text/event-stream

data: {"frame_id":1,"direction":"receive","frame_type":"text",...}

data: {"frame_id":2,"direction":"send","frame_type":"text",...}
```

**应用场景**: 实时监控 WebSocket 连接的消息（WebSocket 专用）。

### 2.10 取消订阅帧流

```
DELETE /api/traffic/{id}/frames/unsubscribe
```

### 2.11 订阅 SSE 事件流 (SSE)

```
GET /api/traffic/{id}/sse/stream?from=begin
```

**说明**:

- 仅对 `is_sse=true` 的流量记录生效
- open 连接会返回 `text/event-stream`，先输出已落库的历史事件（来自 `response-body` 解析），再持续输出实时增量
- closed 连接返回 `409`，请改用 `GET /api/traffic/{id}/response-body` 拉取完整文本并在前端以 Events 模式解析渲染

**响应**:

```
Content-Type: text/event-stream

data: {"seq":1,"ts":1700000000000,"id":"1","event":"message","data":"...","raw":"id: 1\\nevent: message\\ndata: ...\\n\\n"}

data: {"seq":2,"ts":1700000000100,"id":"2","event":"message","data":"...","raw":"id: 2\\nevent: message\\ndata: ...\\n\\n"}
```

---

## 3. Metrics API - 指标监控

用于获取代理服务的性能指标和统计数据。

### 3.1 获取当前指标

```
GET /api/metrics
```

**响应**:

```json
{
  "timestamp": 1700000000000,
  "memory_used": 52428800,
  "memory_total": 17179869184,
  "cpu_usage": 5.2,
  "total_requests": 1000,
  "active_connections": 50,
  "bytes_sent": 1048576,
  "bytes_received": 2097152,
  "bytes_sent_rate": 10240.5,
  "bytes_received_rate": 20480.3,
  "qps": 15.5,
  "max_qps": 100.0,
  "max_bytes_sent_rate": 102400.0,
  "max_bytes_received_rate": 204800.0,
  "http": {
    "requests": 500,
    "bytes_sent": 524288,
    "bytes_received": 1048576,
    "active_connections": 20
  },
  "https": {
    "requests": 400,
    "bytes_sent": 419430,
    "bytes_received": 838860,
    "active_connections": 25
  },
  "tunnel": {
    "requests": 50,
    "bytes_sent": 52428,
    "bytes_received": 104857,
    "active_connections": 3
  },
  "ws": {
    "requests": 20,
    "bytes_sent": 20971,
    "bytes_received": 41943,
    "active_connections": 1
  },
  "wss": {
    "requests": 30,
    "bytes_sent": 31457,
    "bytes_received": 62914,
    "active_connections": 1
  },
  "h3": {
    "requests": 0,
    "bytes_sent": 0,
    "bytes_received": 0,
    "active_connections": 0
  },
  "socks5": {
    "requests": 0,
    "bytes_sent": 0,
    "bytes_received": 0,
    "active_connections": 0
  }
}
```

**应用场景**: 实时监控面板展示。

### 3.2 获取历史指标

```
GET /api/metrics/history
```

**查询参数**:

- `limit`: 返回历史记录数量

**响应**: 历史指标数组。

**应用场景**: 绘制指标趋势图。

### 3.3 获取应用指标统计

```
GET /api/metrics/apps
```

**响应**:

```json
[
  {
    "app_name": "Safari",
    "requests": 500,
    "active_connections": 10,
    "bytes_sent": 524288,
    "bytes_received": 1048576,
    "http_requests": 200,
    "https_requests": 250,
    "tunnel_requests": 30,
    "ws_requests": 10,
    "wss_requests": 10,
    "h3_requests": 0,
    "socks5_requests": 0
  }
]
```

**应用场景**: 按客户端应用统计流量数据。

### 3.4 获取主机指标统计

```
GET /api/metrics/hosts
```

**响应**:

```json
[
  {
    "host": "example.com",
    "requests": 300,
    "active_connections": 5,
    "bytes_sent": 314572,
    "bytes_received": 629145,
    "http_requests": 100,
    "https_requests": 180,
    "tunnel_requests": 10,
    "ws_requests": 5,
    "wss_requests": 5,
    "h3_requests": 0,
    "socks5_requests": 0
  }
]
```

**应用场景**: 按目标主机统计流量数据。

---

## 4. System API - 系统信息

用于获取代理服务的系统状态和配置信息。

### 4.1 获取系统信息

```
GET /api/system
```

**响应**:

```json
{
  "version": "0.1.0",
  "device_name": "eden-work",
  "os": "macos",
  "arch": "aarch64",
  "cpu_logical_cores": 12,
  "cpu_physical_cores": 12,
  "memory_total_bytes": 51539607552,
  "memory_available_bytes": 17179869184,
  "storage_total_bytes": 994662584320,
  "storage_available_bytes": 412316860416,
  "storage_mount_point": "/",
  "uptime_secs": 3600,
  "pid": 12345
}
```

### 4.2 获取系统概览

```
GET /api/system/overview
```

**响应**:

```json
{
  "system": {
    "version": "0.1.0",
    "device_name": "eden-work",
    "os": "macos",
    "arch": "aarch64",
    "cpu_logical_cores": 12,
    "cpu_physical_cores": 12,
    "memory_total_bytes": 51539607552,
    "memory_available_bytes": 17179869184,
    "storage_total_bytes": 994662584320,
    "storage_available_bytes": 412316860416,
    "storage_mount_point": "/",
    "uptime_secs": 3600,
    "pid": 12345
  },
  "metrics": {...},
  "rules": {
    "total": 5,
    "enabled": 3
  },
  "traffic": {
    "recorded": 1000
  },
  "server": {
    "port": 9900,
    "admin_url": "http://127.0.0.1:9900/_bifrost/"
  },
  "pending_authorizations": 0
}
```

**应用场景**: Web UI 首页仪表盘展示。

---

## 5. Values API - 变量管理

用于管理可在规则中引用的变量值。

### 5.1 获取变量列表

```
GET /api/values
```

**响应**:

```json
{
  "values": [
    {
      "name": "API_HOST",
      "value": "api.example.com"
    }
  ],
  "total": 1
}
```

### 5.2 创建变量

```
POST /api/values
```

**请求体**:

```json
{
  "name": "API_HOST",
  "value": "api.example.com"
}
```

### 5.3 获取变量

```
GET /api/values/{name}
```

### 5.4 更新变量

```
PUT /api/values/{name}
```

**请求体**:

```json
{
  "value": "new-value"
}
```

### 5.5 删除变量

```
DELETE /api/values/{name}
```

**应用场景**: 管理规则中使用的动态变量，如环境切换、API 地址配置等。

---

## 6. Whitelist API - 访问控制

用于管理客户端 IP 白名单和访问授权。

### 6.1 获取白名单

```
GET /api/whitelist
```

**响应**:

```json
{
  "mode": "whitelist",
  "allow_lan": true,
  "whitelist": ["192.168.1.0/24", "10.0.0.1"],
  "temporary_whitelist": ["192.168.1.100"]
}
```

| 字段                | 类型     | 说明                                                 |
| ------------------- | -------- | ---------------------------------------------------- |
| mode                | string   | 访问模式: allow_all/local_only/whitelist/interactive |
| allow_lan           | boolean  | 是否允许局域网访问                                   |
| whitelist           | string[] | 永久白名单（IP 或 CIDR）                             |
| temporary_whitelist | string[] | 临时白名单（会话级）                                 |

### 6.2 添加白名单

```
POST /api/whitelist
```

**请求体**:

```json
{
  "ip_or_cidr": "192.168.1.0/24"
}
```

### 6.3 移除白名单

```
DELETE /api/whitelist
```

**请求体**:

```json
{
  "ip_or_cidr": "192.168.1.0/24"
}
```

### 6.4 获取访问模式

```
GET /api/whitelist/mode
```

**响应**:

```json
{
  "mode": "whitelist"
}
```

### 6.5 设置访问模式

```
PUT /api/whitelist/mode
```

**请求体**:

```json
{
  "mode": "whitelist"
}
```

**模式说明**:

- `open`: 允许所有连接
- `whitelist`: 仅允许白名单 IP
- `strict`: 严格模式

### 6.6 获取 LAN 访问设置

```
GET /api/whitelist/allow-lan
```

### 6.7 设置 LAN 访问

```
PUT /api/whitelist/allow-lan
```

**请求体**:

```json
{
  "allow_lan": true
}
```

### 6.8 添加临时白名单

```
POST /api/whitelist/temporary
```

**请求体**:

```json
{
  "ip": "192.168.1.100"
}
```

**应用场景**: 临时授权某个 IP 访问，重启后失效。

### 6.9 移除临时白名单

```
DELETE /api/whitelist/temporary
```

**请求体**:

```json
{
  "ip": "192.168.1.100"
}
```

### 6.10 获取待授权列表

```
GET /api/whitelist/pending
```

**响应**: 等待授权的 IP 列表。

**应用场景**: 当有新 IP 尝试连接时，可以在 Web UI 中审批。

### 6.11 订阅待授权事件 (SSE)

```
GET /api/whitelist/pending/stream
```

**响应**: Server-Sent Events 流，实时推送授权事件。

**应用场景**: 实时监听新的访问授权请求。

### 6.12 批准授权

```
POST /api/whitelist/pending/approve
```

**请求体**:

```json
{
  "ip": "192.168.1.100"
}
```

### 6.13 拒绝授权

```
POST /api/whitelist/pending/reject
```

**请求体**:

```json
{
  "ip": "192.168.1.100"
}
```

### 6.14 清空待授权列表

```
DELETE /api/whitelist/pending
```

---

## 7. Cert API - 证书管理

用于 HTTPS 代理的 CA 证书下载和管理。

### 7.1 获取证书信息

```
GET /api/cert/info
```

**响应**:

```json
{
  "available": true,
  "local_ips": ["192.168.1.100", "10.0.0.1"],
  "download_urls": [
    "http://192.168.1.100:9900/_bifrost/public/cert",
    "http://10.0.0.1:9900/_bifrost/public/cert"
  ],
  "qrcode_urls": ["http://192.168.1.100:9900/_bifrost/public/cert/qrcode"]
}
```

**应用场景**: Web UI 展示证书下载链接和二维码。

### 7.2 下载 CA 证书

```
GET /public/cert
```

**响应**:

- Content-Type: `application/x-pem-file`
- Content-Disposition: `attachment; filename="bifrost-ca.crt"`

**应用场景**: 移动设备或浏览器下载并安装 CA 证书以支持 HTTPS 代理。

### 7.3 获取证书下载二维码

```
GET /public/cert/qrcode
```

**查询参数**:

- `ip`: 指定 IP 地址（可选）

**响应**: SVG 格式的二维码图片。

**应用场景**: 移动设备扫码下载证书。

---

## 8. Proxy API - 系统代理

用于管理操作系统级别的代理设置。

### 8.1 获取系统代理状态

```
GET /api/proxy/system
```

**响应**:

```json
{
  "supported": true,
  "enabled": true,
  "host": "127.0.0.1",
  "port": 9900,
  "bypass": "localhost,127.0.0.1,::1,*.local"
}
```

### 8.2 设置系统代理

```
PUT /api/proxy/system
```

**请求体**:

```json
{
  "enabled": true,
  "bypass": "localhost,127.0.0.1,::1,*.local"
}
```

**注意**: 在某些系统上可能需要管理员权限。

**错误响应**（需要管理员权限时）:

```json
{
  "error": "requires_admin",
  "message": "System proxy requires administrator privileges..."
}
```

**用户取消授权时**:

```json
{
  "error": "user_cancelled",
  "message": "Authorization was cancelled by user."
}
```

### 8.3 获取系统代理支持状态

```
GET /api/proxy/system/support
```

**响应**:

```json
{
  "supported": true,
  "platform": "macOS"
}
```

### 8.4 获取 CLI 代理状态（环境变量）

### 8.4 获取 macOS 系统代理 cleanup LaunchDaemon 状态

```
GET /api/proxy/system/launchd
```

**响应**:

```json
{
  "supported": true,
  "installed": true,
  "loaded": true,
  "label": "com.bifrost.system-proxy-cleanup",
  "plist_path": "/Library/LaunchDaemons/com.bifrost.system-proxy-cleanup.plist",
  "program": "/usr/local/bin/bifrost",
  "data_dir": "/Users/me/.bifrost",
  "installed_version": "0.0.88",
  "current_version": "0.0.88",
  "needs_upgrade": false,
  "message": null
}
```

### 8.5 安装或卸载 macOS 系统代理 cleanup LaunchDaemon

```
PUT /api/proxy/system/launchd
```

**请求体**:

```json
{
  "enabled": true
}
```

**说明**:
- macOS 上安装/卸载需要管理员授权，服务端会通过系统 GUI 授权窗口请求密码或指纹。
- 用户取消授权时返回 `user_cancelled`，Bifrost 主服务继续运行。
- 已安装且版本一致时，启动流程不会重复安装；版本不一致时状态返回 `needs_upgrade=true`，再次启用会重新安装当前版本。

**错误响应**（用户取消授权时）:

```json
{
  "error": "user_cancelled",
  "message": "Authorization was cancelled by user."
}
```

### 8.6 获取 CLI 代理状态（环境变量）

用于展示命令行代理（写入 shell rc 文件的 http_proxy 等变量）是否启用。

```
GET /api/proxy/cli
```

**响应**:

```json
{
  "enabled": true,
  "shell": "zsh",
  "config_files": ["~/.zshrc", "~/.zprofile"],
  "proxy_url": "http://127.0.0.1:9900"
}
```

### 8.5 获取代理地址信息

```
GET /api/proxy/address
```

**响应**:

```json
{
  "port": 9900,
  "local_ips": ["192.168.1.100", "10.0.0.1"],
  "addresses": [
    {
      "ip": "192.168.1.100",
      "address": "192.168.1.100:9900",
      "qrcode_url": "/_bifrost/public/proxy/qrcode?ip=192.168.1.100"
    }
  ]
}
```

**应用场景**: 获取代理地址供客户端配置。

### 8.6 获取代理地址二维码

```
GET /public/proxy/qrcode
```

**查询参数**:

- `ip`: 指定 IP 地址（可选）

**响应**: SVG 格式的二维码图片，包含代理地址。

**应用场景**: 移动设备扫码配置代理。

---

## 9. Config API - 配置管理

用于管理代理服务的运行时配置。

### 9.1 获取代理设置

```
GET /api/config
```

**响应**:

```json
{
  "tls": {
    "enable_tls_interception": true,
    "intercept_exclude": ["*.apple.com"],
    "intercept_include": [],
    "app_intercept_exclude": ["Finder"],
    "app_intercept_include": [],
    "unsafe_ssl": false,
    "disconnect_on_config_change": true
  },
  "port": 9900,
  "host": "127.0.0.1"
}
```

### 9.2 获取 TLS 配置

```
GET /api/config/tls
```

**响应**:

```json
{
  "enable_tls_interception": true,
  "intercept_exclude": ["*.apple.com"],
  "intercept_include": [],
  "app_intercept_exclude": ["Finder"],
  "app_intercept_include": [],
  "unsafe_ssl": false,
  "disconnect_on_config_change": true
}
```

### 9.3 更新 TLS 配置

```
PUT /api/config/tls
```

**请求体**:

```json
{
  "enable_tls_interception": true,
  "intercept_exclude": ["*.apple.com", "*.google.com"],
  "intercept_include": [],
  "app_intercept_exclude": [],
  "app_intercept_include": [],
  "unsafe_ssl": false,
  "disconnect_on_config_change": true
}
```

| 字段                        | 类型     | 必填 | 说明                             |
| --------------------------- | -------- | ---- | -------------------------------- |
| enable_tls_interception     | boolean  | 否   | 是否启用 TLS 拦截                |
| intercept_exclude           | string[] | 否   | 排除的域名模式列表               |
| intercept_include           | string[] | 否   | 包含的域名模式列表               |
| app_intercept_exclude       | string[] | 否   | 排除的应用名称列表               |
| app_intercept_include       | string[] | 否   | 包含的应用名称列表               |
| unsafe_ssl                  | boolean  | 否   | 是否跳过 SSL 证书验证            |
| disconnect_on_config_change | boolean  | 否   | 配置变更时是否断开受影响的连接   |

**应用场景**: 动态调整 TLS 拦截配置，无需重启服务。

### 9.4 获取性能配置

```
GET /api/config/performance
```

**响应**:

```json
{
  "traffic": {
    "max_records": 5000,
    "max_db_size_bytes": 2147483648,
    "max_body_memory_size": 524288,
    "max_body_buffer_size": 10485760,
    "max_body_probe_size": 65536,
    "file_retention_days": 7,
    "sse_stream_flush_bytes": 262144,
    "sse_stream_flush_interval_ms": 1000,
    "ws_payload_flush_bytes": 524288,
    "ws_payload_flush_interval_ms": 1000,
    "ws_payload_max_open_files": 128
  },
  "body_store_stats": {
    "memory_used": 1048576,
    "file_count": 100,
    "total_size": 10485760
  },
  "traffic_store_stats": {
    "total_records": 5000,
    "total_records_processed": 10000
  },
  "frame_store_stats": {
    "total_frames": 1000,
    "total_size": 524288
  },
  "ws_payload_store_stats": {
    "file_count": 50,
    "total_size": 1048576
  }
}
```

### 9.5 更新性能配置

```
PUT /api/config/performance
```

**请求体**:

```json
{
  "max_records": 10000,
  "max_db_size_bytes": 2147483648,
  "max_body_memory_size": 1048576,
  "max_body_buffer_size": 20971520,
  "max_body_probe_size": 65536,
  "file_retention_days": 3,
  "sse_stream_flush_bytes": 262144,
  "sse_stream_flush_interval_ms": 1000,
  "ws_payload_flush_bytes": 524288,
  "ws_payload_flush_interval_ms": 1000,
  "ws_payload_max_open_files": 128
}
```

| 字段                         | 类型   | 必填 | 说明                            |
| ---------------------------- | ------ | ---- | ------------------------------- |
| max_records                  | number | 否   | 最大流量记录数                  |
| max_db_size_bytes            | number | 否   | Traffic 数据总大小上限（字节，包含 body_cache/frames/ws_payload） |
| max_body_memory_size         | number | 否   | 单个请求体最大内存缓存大小      |
| max_body_buffer_size         | number | 否   | 请求体缓冲区最大大小            |
| max_body_probe_size          | number | 否   | 非文本/疑似大流量 body 预读探测上限（超过则跳过 body 处理并直接流式转发） |
| file_retention_days          | number | 否   | 文件保留天数（最大 7 天）       |
| sse_stream_flush_bytes       | number | 否   | SSE raw stream flush 字节阈值   |
| sse_stream_flush_interval_ms | number | 否   | SSE raw stream flush 间隔（ms） |
| ws_payload_flush_bytes       | number | 否   | WS payload flush 字节阈值       |
| ws_payload_flush_interval_ms | number | 否   | WS payload flush 间隔（ms）     |
| ws_payload_max_open_files    | number | 否   | WS payload 最大打开文件数       |

### 9.6 清除缓存

```
DELETE /api/config/performance/clear-cache
```

**响应**:

```json
{
  "body_cache_removed": 100,
  "traffic_cache_removed": 5000,
  "frame_cache_removed": 500,
  "ws_payload_cache_removed": 50,
  "message": "Successfully cleared 100 body cache files, 5000 traffic records, 500 frame files, and 50 ws payload files"
}
```

**应用场景**: 释放磁盘空间，清理历史数据。

### 9.7 按域名断开连接

```
POST /api/config/connections/disconnect
```

**请求体**:

```json
{
  "domain": "example.com"
}
```

**响应**:

```json
{
  "success": true,
  "disconnected_count": 5,
  "message": "Disconnected 5 connection(s) matching 'example.com'"
}
```

**应用场景**: 强制断开特定域名的所有连接，用于调试或清理。

---

## 10. WebSocket Connections API

### 10.1 获取 WebSocket 连接列表

```
GET /api/websocket/connections
```

**响应**:

```json
{
  "connections": [
    {
      "id": "req-123",
      "frame_count": 100,
      "socket_status": {
        "is_open": true,
        "send_count": 50,
        "receive_count": 50,
        "send_bytes": 5000,
        "receive_bytes": 10000,
        "frame_count": 100
      },
      "is_monitored": true
    }
  ],
  "total": 1
}
```

**应用场景**: 查看当前活跃的 WebSocket 连接。

---

## 11. Push API - WebSocket 推送

用于 Web UI 实时数据推送。

### 11.1 建立 WebSocket 连接

```
GET /api/push (WebSocket Upgrade)
```

**查询参数**:

| 参数            | 类型    | 说明                   |
| --------------- | ------- | ---------------------- |
| last_traffic_id | string  | 最后获取的流量记录 ID  |
| pending_ids     | string  | 待更新的记录 ID 列表   |
| need_overview   | boolean | 是否需要系统概览       |
| need_metrics    | boolean | 是否需要指标数据       |
| need_history    | boolean | 是否需要历史指标       |
| history_limit   | number  | 历史指标数量限制       |

**推送消息类型**:

```json
{
  "type": "connected",
  "data": {
    "client_id": 1,
    "message": "WebSocket connection established"
  }
}
```

```json
{
  "type": "traffic_update",
  "data": {
    "new_records": [...],
    "updated_records": [...]
  }
}
```

```json
{
  "type": "metrics",
  "data": {...}
}
```

**应用场景**: Web UI 实时接收流量、指标等更新。

---

## 12. App Icon API - 应用图标

用于获取客户端应用程序图标（仅 macOS）。

### 12.1 获取应用图标

```
GET /api/app-icon/{app_name}
```

**路径参数**:

- `app_name`: 应用名称（需 URL 编码）

**响应**:

- Content-Type: `image/png`
- 成功返回 PNG 格式图标
- 失败返回 404

**应用场景**: Web UI 显示客户端应用图标。

---

## 数据结构参考

### TrafficRecord

完整的流量记录，包含请求和响应的所有信息。

| 字段                 | 类型                | 说明              |
| -------------------- | ------------------- | ----------------- |
| id                   | string              | 唯一标识          |
| sequence             | number              | 序列号            |
| timestamp            | number              | 时间戳（毫秒）    |
| method               | string              | HTTP 方法         |
| url                  | string              | 完整 URL          |
| status               | number              | HTTP 状态码       |
| content_type         | string?             | 响应 Content-Type |
| request_content_type | string?             | 请求 Content-Type |
| request_size         | number              | 请求大小（字节）  |
| response_size        | number              | 响应大小（字节）  |
| duration_ms          | number              | 总耗时（毫秒）    |
| timing               | RequestTiming?      | 详细耗时          |
| request_headers      | [string, string][]? | 请求头            |
| response_headers     | [string, string][]? | 响应头            |
| client_ip            | string              | 客户端 IP         |
| client_app           | string?             | 客户端应用名称    |
| client_pid           | number?             | 客户端进程 ID     |
| client_path          | string?             | 客户端应用路径    |
| host                 | string              | 主机名            |
| path                 | string              | 路径              |
| protocol             | string              | 协议 (http/https) |
| is_tunnel            | boolean             | 是否为隧道连接    |
| is_h3                | boolean             | 是否为 HTTP/3     |
| has_rule_hit         | boolean             | 是否命中规则      |
| matched_rules        | MatchedRule[]?      | 命中的规则列表    |
| is_websocket         | boolean             | 是否为 WebSocket  |
| is_sse               | boolean             | 是否为 SSE        |
| socket_status        | SocketStatus?       | Socket 状态       |
| frame_count          | number              | 帧数量            |
| last_frame_id        | number              | 最后帧 ID         |

### TrafficSummary

流量摘要，用于列表展示。

| 字段               | 类型          | 说明             |
| ------------------ | ------------- | ---------------- |
| id                 | string        | 唯一标识         |
| sequence           | number        | 序列号           |
| timestamp          | number        | 时间戳（毫秒）   |
| method             | string        | HTTP 方法        |
| url                | string        | 完整 URL         |
| status             | number        | HTTP 状态码      |
| content_type       | string?       | 响应 Content-Type|
| request_size       | number        | 请求大小（字节） |
| response_size      | number        | 响应大小（字节） |
| duration_ms        | number        | 总耗时（毫秒）   |
| host               | string        | 主机名           |
| path               | string        | 路径             |
| protocol           | string        | 协议             |
| client_ip          | string        | 客户端 IP        |
| client_app         | string?       | 客户端应用名称   |
| client_pid         | number?       | 客户端进程 ID    |
| client_path        | string?       | 客户端应用路径   |
| has_rule_hit       | boolean       | 是否命中规则     |
| matched_rule_count | number        | 命中规则数量     |
| matched_protocols  | string[]      | 命中的协议列表   |
| is_websocket       | boolean       | 是否为 WebSocket |
| is_sse             | boolean       | 是否为 SSE       |
| is_h3              | boolean       | 是否为 HTTP/3    |
| is_tunnel          | boolean       | 是否为隧道连接   |
| frame_count        | number        | 帧数量           |
| socket_status      | SocketStatus? | Socket 状态      |
| start_time         | string        | 开始时间（格式化）|
| end_time           | string?       | 结束时间（格式化）|

### RequestTiming

请求各阶段耗时。

| 字段       | 类型    | 说明         |
| ---------- | ------- | ------------ |
| dns_ms     | number? | DNS 解析耗时 |
| connect_ms | number? | TCP 连接耗时 |
| tls_ms     | number? | TLS 握手耗时 |
| send_ms    | number? | 发送请求耗时 |
| wait_ms    | number? | 等待响应耗时 |
| receive_ms | number? | 接收响应耗时 |
| total_ms   | number  | 总耗时       |

### MatchedRule

命中的规则信息。

| 字段      | 类型    | 说明         |
| --------- | ------- | ------------ |
| pattern   | string  | 匹配模式     |
| protocol  | string  | 规则协议     |
| value     | string  | 规则值       |
| rule_name | string? | 规则文件名称 |
| raw       | string? | 原始规则文本 |
| line      | number? | 规则所在行号 |

### SocketStatus

WebSocket/SSE 连接状态。

| 字段          | 类型    | 说明         |
| ------------- | ------- | ------------ |
| is_open       | boolean | 连接是否打开 |
| send_count    | number  | 发送消息数   |
| receive_count | number  | 接收消息数   |
| send_bytes    | number  | 发送字节数   |
| receive_bytes | number  | 接收字节数   |
| frame_count   | number  | 总帧数       |
| close_code    | number? | 关闭码       |
| close_reason  | string? | 关闭原因     |

### MetricsSnapshot

指标快照。

| 字段                   | 类型               | 说明                 |
| ---------------------- | ------------------ | -------------------- |
| timestamp              | number             | 时间戳（毫秒）       |
| memory_used            | number             | 内存使用量（字节）   |
| memory_total           | number             | 系统总内存（字节）   |
| cpu_usage              | number             | CPU 使用率（%）      |
| total_requests         | number             | 总请求数             |
| active_connections     | number             | 活跃连接数           |
| bytes_sent             | number             | 发送字节数           |
| bytes_received         | number             | 接收字节数           |
| bytes_sent_rate        | number             | 发送速率（字节/秒）  |
| bytes_received_rate    | number             | 接收速率（字节/秒）  |
| qps                    | number             | 每秒请求数           |
| max_qps                | number             | 历史最大 QPS         |
| max_bytes_sent_rate    | number             | 历史最大发送速率     |
| max_bytes_received_rate| number             | 历史最大接收速率     |
| http                   | TrafficTypeMetrics | HTTP 流量指标        |
| https                  | TrafficTypeMetrics | HTTPS 流量指标       |
| tunnel                 | TrafficTypeMetrics | 隧道流量指标         |
| ws                     | TrafficTypeMetrics | WebSocket 流量指标   |
| wss                    | TrafficTypeMetrics | WSS 流量指标         |
| h3                     | TrafficTypeMetrics | HTTP/3 流量指标      |
| socks5                 | TrafficTypeMetrics | SOCKS5 流量指标      |

### TrafficTypeMetrics

按协议类型的流量指标。

| 字段               | 类型   | 说明       |
| ------------------ | ------ | ---------- |
| requests           | number | 请求数     |
| bytes_sent         | number | 发送字节数 |
| bytes_received     | number | 接收字节数 |
| active_connections | number | 活跃连接数 |

### FrameDirection

帧方向枚举。

| 值      | 说明 |
| ------- | ---- |
| send    | 发送 |
| receive | 接收 |

### FrameType

帧类型枚举。

| 值           | 说明     |
| ------------ | -------- |
| text         | 文本帧   |
| binary       | 二进制帧 |
| ping         | Ping 帧  |
| pong         | Pong 帧  |
| close        | 关闭帧   |
| continuation | 续帧     |
| sse          | SSE 事件 |

### TlsConfig

TLS 配置。

| 字段                        | 类型     | 说明                           |
| --------------------------- | -------- | ------------------------------ |
| enable_tls_interception     | boolean  | 是否启用 TLS 拦截              |
| intercept_exclude           | string[] | 排除的域名模式列表             |
| intercept_include           | string[] | 包含的域名模式列表             |
| app_intercept_exclude       | string[] | 排除的应用名称列表             |
| app_intercept_include       | string[] | 包含的应用名称列表             |
| unsafe_ssl                  | boolean  | 是否跳过 SSL 证书验证          |
| disconnect_on_config_change | boolean  | 配置变更时是否断开受影响的连接 |
