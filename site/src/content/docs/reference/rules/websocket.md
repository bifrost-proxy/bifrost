---
title: "WebSocket"
description: "WebSocket 相关规则与使用说明。"
editUrl: false
sidebar:
  label: "WebSocket"
  order: 290
---

> 此页面由 `docs/rules/websocket.md` 自动同步生成。
# WebSocket 规则

本章介绍 WebSocket 请求的转发和代理规则。

---

## ws

将 WebSocket 请求转换为新的 `ws://` 请求（服务端将收到转换后的 WebSocket URL）。

> ⚠️ **注意**：只支持 WebSocket 请求 `ws[s]://domain[:port]/[path][?query]`，不支持转换隧道代理和普通 HTTP/HTTPS

### 语法

```
pattern ws://target_host[:port][/path]
```

### 示例

#### 基础转发

```bash
# 将 ws 请求转发到另一个服务器
ws://www.example.com/socket ws://ws-server.example.com/socket

# 将 wss 请求降级为 ws
wss://www.example.com/chat ws://internal-ws.example.com/chat
```

### 路径拼接规则

#### 1. 自动路径拼接（默认行为）

```bash
ws://www.example.com/path1 ws://www.test.com/path/xxx
wss://www.example.com/path2 ws://www.abc.com/path3/yyy
```

| 原始请求                                  | 转换结果（服务端收到的 URL）             |
| ----------------------------------------- | ---------------------------------------- |
| `ws://www.example.com/path1`              | `ws://www.test.com/path/xxx`             |
| `ws://www.example.com/path1/a/b/c?query`  | `ws://www.test.com/path/xxx/a/b/c?query` |
| `wss://www.example.com/path2`             | `ws://www.abc.com/path3/yyy`             |
| `wss://www.example.com/path2/a/b/c?query` | `ws://www.abc.com/path3/yyy/a/b/c?query` |

### 非 WebSocket 请求的处理

| 请求类型           | 匹配 `ws` 规则的结果 |
| ------------------ | -------------------- |
| WebSocket 请求     | 正常转发             |
| 隧道代理请求       | 非拦截（passthrough）隧道忽略匹配；TLS 拦截隧道内的 WebSocket 升级仍会被匹配并改写 |
| 普通 HTTP/HTTPS    | `ws://` 作为目标时按普通 HTTP 请求转发（不会返回 502）；`ws://` 作为 pattern 时不匹配普通 HTTP |

### 测试用例

| 测试场景         | 规则                                           | 预期                                     |
| ---------------- | ---------------------------------------------- | ---------------------------------------- |
| ws 转发          | `ws://test.com ws://target.com`                | 转发到 `ws://target.com`                 |
| wss 降级为 ws    | `wss://test.com ws://target.com`               | 转发到 `ws://target.com`                 |
| 路径自动拼接     | `ws://test.com/api ws://target.com/v2`         | `/api/xxx` → `/v2/xxx`                   |
| HTTP 请求        | `http://test.com ws://target.com`              | 作为普通 HTTP 请求转发，返回 200         |

---

## wss

将 WebSocket 请求转换为新的 `wss://` 请求（服务端将收到转换后的 WebSocket Secure URL）。

> ⚠️ **注意**：只支持 WebSocket 请求 `ws[s]://domain[:port]/[path][?query]`，不支持转换隧道代理和普通 HTTP/HTTPS

### 语法

```
pattern wss://target_host[:port][/path]
```

### 示例

#### 基础转发

```bash
# 将 ws 请求升级为 wss
ws://www.example.com/socket wss://secure-ws.example.com/socket

# 将 wss 请求转发到另一个安全服务器
wss://www.example.com/chat wss://wss-server.example.com/chat
```

### 路径拼接规则

与 `ws` 规则相同：

#### 1. 自动路径拼接（默认行为）

```bash
ws://www.example.com/path1 wss://www.test.com/path/xxx
wss://www.example.com/path2 wss://www.abc.com/path3/yyy
```

| 原始请求                                    | 转换结果（服务端收到的 URL）                  |
| ------------------------------------------- | --------------------------------------------- |
| `ws://www.example.com/path1`                | `wss://www.test.com/path/xxx`                 |
| `ws://www.example.com/path1/a/b/c?query`    | `wss://www.test.com/path/xxx/a/b/c?query`     |
| `wss://www.example.com/path2`               | `wss://www.abc.com/path3/yyy`                 |
| `wss://www.example.com/path2/a/b/c?query`   | `wss://www.abc.com/path3/yyy/a/b/c?query`     |

### 非 WebSocket 请求的处理

| 请求类型           | 匹配 `wss` 规则的结果 |
| ------------------ | --------------------- |
| WebSocket 请求     | 正常转发              |
| 隧道代理请求       | 非拦截（passthrough）隧道忽略匹配；TLS 拦截隧道内的 WebSocket 升级仍会被匹配并改写 |
| 普通 HTTP/HTTPS    | `wss://` 作为目标时按普通 HTTP 请求转发（向非 TLS 上游 TLS 握手失败时才返回 502）；`wss://` 作为 pattern 时不匹配普通 HTTP |

### 测试用例

| 测试场景         | 规则                                           | 预期                                       |
| ---------------- | ---------------------------------------------- | ------------------------------------------ |
| ws 升级为 wss    | `ws://test.com wss://target.com`               | 转发到 `wss://target.com`                  |
| wss 转发         | `wss://test.com wss://target.com`              | 转发到 `wss://target.com`                  |
| 路径自动拼接     | `wss://test.com/api wss://target.com/v2`       | `/api/xxx` → `/v2/xxx`                     |
| HTTP 请求        | `http://test.com wss://target.com`             | 作为普通 HTTP 请求转发（TLS 握手失败时返回 502） |

---

## ws vs wss 对比

| 特性             | ws                           | wss                          |
| ---------------- | ---------------------------- | ---------------------------- |
| 协议             | WebSocket (非加密)           | WebSocket Secure (TLS 加密)  |
| 默认端口         | 80                           | 443                          |
| 安全性           | 无加密                       | TLS 加密                     |
| 适用场景         | 内网/开发环境                | 生产环境/公网                |
| 证书要求         | 无                           | 需要有效 TLS 证书            |

---

## 使用场景

### 1. 开发环境代理

将生产环境的 WebSocket 请求代理到本地开发服务器：

```bash
# 生产环境 wss 降级到本地 ws
wss://api.example.com/ws ws://localhost:8080/ws
```

### 2. 测试环境切换

```bash
# 切换 WebSocket 服务器
ws://www.example.com/socket ws://test-server.example.com/socket
wss://www.example.com/socket wss://test-server.example.com/socket
```

### 3. 负载均衡测试

```bash
# 将请求路由到不同后端
ws://www.example.com/socket ws://ws-server-1.example.com/socket
# ws://www.example.com/socket ws://ws-server-2.example.com/socket
```

### 4. 协议升级/降级

```bash
# 本地开发时降级为 ws（避免证书问题）
wss://api.example.com/realtime ws://localhost:3000/realtime

# 测试 TLS 配置
ws://api.example.com/realtime wss://secure-api.example.com/realtime
```

### 5. 结合其他规则

```bash
# WebSocket 转发 + 请求头修改
wss://api.example.com/ws wss://internal-ws.example.com/ws reqHeaders://(X-Forwarded-For:client-ip)

# WebSocket 转发 + 延迟（注意：reqDelay 等请求时序变换作用于 HTTP 请求/响应阶段，
# 不会作用于 WebSocket 握手/升级，握手会立即完成）
ws://api.example.com/socket ws://test-server.example.com/socket reqDelay://1000
```

---

## 注意事项

1. **仅支持 WebSocket**：`ws` 和 `wss` 规则的路径拼接等转换仅对 WebSocket 升级请求生效；普通 HTTP/HTTPS 请求若匹配到目标为 `ws://` 的规则，会按普通 HTTP 请求转发（目标为 `wss://` 时仅在向非 TLS 上游 TLS 握手失败时返回 502），而非刻意拒绝
2. **协议转换**：可以在 `ws` 和 `wss` 之间相互转换
3. **路径处理**：默认会自动拼接剩余路径
4. **查询参数**：自动保留原始请求的查询参数
5. **隧道代理**：只有非拦截（passthrough）的 CONNECT 隧道会忽略这些规则；隧道一旦被 TLS 拦截，其内部的 WebSocket 升级仍会被匹配并改写

---

## 关联协议

- [host](../routing/#host) - HTTP/HTTPS 请求转发
- [proxy](../routing/#proxy) - HTTP 代理转发
