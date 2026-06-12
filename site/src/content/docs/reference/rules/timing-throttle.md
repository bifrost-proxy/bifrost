---
title: "流量控制"
description: "延迟、限速与相关节流能力说明。"
editUrl: false
sidebar:
  label: "流量控制"
  order: 260
---

> 此页面由 `docs/rules/timing-throttle.md` 自动同步生成。
# 延迟与限速规则

本章介绍模拟网络延迟和带宽限制的规则。

---

## reqDelay

在发送请求之前添加延迟。

> **重要：reqDelay 仅对 TLS 拦截（HTTPS）流量生效。** 它只在拦截路径上被应用，对普通 HTTP 请求当前是空操作（会被静默忽略）。依赖 reqDelay 时，请使用启用了 `tlsIntercept` 的 HTTPS 域名；若只需在普通 HTTP 上模拟请求侧变慢，请改用 `reqSpeed`（上传限速，对普通 HTTP 有效）。`resDelay`/`reqSpeed`/`resSpeed` 在普通 HTTP 上均可正常工作。

### 语法

```
pattern reqDelay://milliseconds
```

### 参数说明

| 参数 | 说明 |
|------|------|
| `milliseconds` | 延迟时间，单位毫秒 |

### 示例

```bash
# 延迟 1 秒
www.example.com reqDelay://1000

# 延迟 500 毫秒
www.example.com/api reqDelay://500

# 延迟 3 秒
www.example.com/slow reqDelay://3000
```

### 使用场景

```bash
# 模拟慢速网络
www.example.com reqDelay://2000

# 测试超时处理
www.example.com/api reqDelay://10000

# 测试加载状态
www.example.com/data reqDelay://1500
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 1 秒延迟 | `test.com reqDelay://1000` | 请求发送延迟 ~1000ms |
| 500ms 延迟 | `test.com reqDelay://500` | 请求发送延迟 ~500ms |

---

## resDelay

在返回响应之前添加延迟。

### 语法

```
pattern resDelay://milliseconds
```

### 示例

```bash
# 响应延迟 1 秒
www.example.com resDelay://1000

# 响应延迟 2 秒
www.example.com/api resDelay://2000
```

### 使用场景

```bash
# 模拟服务器处理延迟
www.example.com/api resDelay://500

# 测试前端 loading 状态
www.example.com/data resDelay://2000

# 测试超时重试
www.example.com/slow resDelay://5000
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 1 秒延迟 | `test.com resDelay://1000` | 响应返回延迟 ~1000ms |
| 2 秒延迟 | `test.com resDelay://2000` | 响应返回延迟 ~2000ms |

---

## reqSpeed

限制请求发送速度（模拟上传带宽限制）。

### 语法

```
pattern reqSpeed://kb_per_second
```

### 参数说明

| 参数 | 说明 |
|------|------|
| `kb_per_second` | 每秒传输千字节数 |

### 示例

```bash
# 限制上传速度为 10 KB/s
www.example.com reqSpeed://10

# 限制上传速度为 100 KB/s
www.example.com reqSpeed://100

# 极慢上传（1 KB/s）
www.example.com reqSpeed://1
```

### 使用场景

```bash
# 测试大文件上传
www.example.com/upload reqSpeed://50

# 模拟 2G 网络上传
www.example.com reqSpeed://5

# 模拟 3G 网络上传
www.example.com reqSpeed://30
```

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 10 KB/s | `test.com reqSpeed://10` | 上传速度限制在 ~10 KB/s |

---

## resSpeed

限制响应接收速度（模拟下载带宽限制）。

### 语法

```
pattern resSpeed://kb_per_second
```

### 示例

```bash
# 限制下载速度为 10 KB/s
www.example.com resSpeed://10

# 限制下载速度为 100 KB/s
www.example.com resSpeed://100

# 极慢下载（1 KB/s）
www.example.com resSpeed://1
```

### 使用场景

```bash
# 测试大文件下载
www.example.com/download resSpeed://50

# 模拟 2G 网络
www.example.com resSpeed://5

# 模拟 3G 网络
www.example.com resSpeed://30

# 测试流式加载
www.example.com/stream resSpeed://20
```

### 常用带宽模拟

| 网络类型 | 速度设置 | 说明 |
|---------|---------|------|
| 2G (GPRS) | `5-10` KB/s | 极慢网络 |
| 2G (EDGE) | `20-30` KB/s | 慢速网络 |
| 3G | `100-300` KB/s | 中速网络 |
| 4G | `1000-5000` KB/s | 快速网络 |
| WiFi | `5000-10000` KB/s | 高速网络 |

### 测试用例

| 测试场景 | 规则 | 预期 |
|---------|------|------|
| 10 KB/s | `test.com resSpeed://10` | 下载速度限制在 ~10 KB/s |
| 100 KB/s | `test.com resSpeed://100` | 下载速度限制在 ~100 KB/s |

---

## 规则组合

延迟和限速规则可以组合使用：

```bash
# 请求延迟 + 响应延迟
www.example.com reqDelay://500 resDelay://1000

# 延迟 + 限速
www.example.com resDelay://500 resSpeed://10

# 完整慢速网络模拟（reqDelay 仅在 HTTPS + tlsIntercept 下生效，普通 HTTP 会忽略该段）
www.example.com reqDelay://200 resDelay://500 reqSpeed://10 resSpeed://20

# 配合路由规则
www.example.com host://backend.local resDelay://1000

# 配合过滤器
www.example.com resDelay://2000 includeFilter://m:POST
```

---

## 网络条件模拟

> **注意：** 以下示例均包含 `reqDelay`，而 `reqDelay` 仅对 TLS 拦截（HTTPS）流量生效。若目标是普通 HTTP 域名，请改用启用 `tlsIntercept` 的 HTTPS 域名，否则其中的 `reqDelay` 段会被静默忽略，请求侧延迟不会生效（响应侧 `resDelay`/限速仍正常）。

### 2G 网络模拟

```bash
www.example.com reqDelay://500 resDelay://1000 reqSpeed://5 resSpeed://10
```

### 3G 网络模拟

```bash
www.example.com reqDelay://100 resDelay://200 reqSpeed://50 resSpeed://100
```

### 4G 网络模拟

```bash
www.example.com reqDelay://50 resDelay://50 reqSpeed://500 resSpeed://1000
```

### 高延迟网络

```bash
www.example.com reqDelay://1000 resDelay://1000
```

---

## 注意事项

1. **延迟单位**：`reqDelay`/`resDelay` 的单位是毫秒
2. **速度单位**：`reqSpeed`/`resSpeed` 的单位是 KB/s
3. **组合效果**：延迟和限速可以叠加使用
4. **测试建议**：测试超时时，确保测试超时时间大于设置的延迟时间
5. **性能影响**：极低的速度设置可能导致长时间等待
